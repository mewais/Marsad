# Phase 2 — Multi-driver support (Codex) — design & block decomposition

**Status:** authoritative plan (grounded in the driver CLIs' real protocols). NOT yet implemented — this
work requires the actual `codex` binary installed to implement and verify against, so it is
scoped here as a set of test-first blocks to land where those CLIs are available (dev machine / CI), NOT in
the headless upgrade sandbox. Claude support already ships (`marsad_agent::claude_argv` + the claude
stream-json parser + claude transcript reader).

## Why this is its own phase (not a one-line argv swap)
`claude_argv` is verified against the real claude 2.1.x binary and threads `LaunchSpec` fields
(model/posture/mode/disallowed_tools) as **CLI flags**. Codex does NOT work that way:

- **Codex** — launched as `codex app-server` (JSON-RPC 2.0 over stdio; `--listen ws://…|unix://…` for
  socket transports, but we want the default stdio). Config (model, approval policy, cwd) and the task are
  delivered **in the protocol** via `initialize` → `initialized`, then a thread + turn; agent output arrives
  as streamed JSON-RPC **notifications**. So `codex_argv` is thin (`["codex","app-server"]`) and the real
  adapter is a JSON-RPC client. Approvals surface as protocol requests (maps cleanly to our HITL gate).

Codex's `app-server` is a JSON-RPC-over-stdio agent protocol, unlike claude's `--print --output-format stream-json`. It
needs a protocol adapter that **normalizes its events into `marsad_events`** (the format the viewmodel and UI
already consume) and maps its approval requests onto our gate — so the rest of Marsad (viewmodel, board,
graph, gates, HITL) is unchanged.

## Prerequisite: driver on `LaunchSpec`
`marsad_proto::LaunchSpec` gains `driver: marsad_workstream::Driver` (claude|codex; default claude).
Every launch path branches on it. The `Driver` enum already exists (marsad-workstream B14) and the UI/graph
already carry it — this closes the request side so a session can be *asked for* per driver.

## Blocks (each test-first, isolated `tests/`, 100% cov like every other block)

| # | Block | Tier | Content |
|---|-------|------|---------|
| P2a | `driver` dispatch + argv | L0 pure | `driver_argv(driver, spec)` over `claude_argv` + `codex_argv` (`["codex","app-server"]`); `LaunchSpec.driver` field + serde. Pure, fully unit-testable now. |
| P2b | Codex app-server client | L2 IO | JSON-RPC 2.0 stdio framing, `initialize`/`initialized` handshake, thread/turn lifecycle, request/notification demux. Trait-mock the transport; the raw socket loop is the thin edge. Needs the real `codex` schema (`codex app-server generate-json-schema`). |
| P2c | Codex→events normalizer | L1 pure | Map codex notifications (agent message/tool-call/approval/turn-complete) → `marsad_events::*`; map codex approval requests → our gate. Pure, table-driven, 100%-testable against captured fixtures. |
| P2f | Resume/transcript per driver | L1/L2 | claude reads its projects JSONL; codex exposes history via its protocol — a `TranscriptSource` per driver behind the existing trait seam. |
| P2g | Launcher wiring | L2 | Branch spawn/IPC/parse on `driver`; codex spawns its app-server process and pump the adapter instead of the claude stream parser. |

**DAG:** `P2a` first (unblocks requests). `P2b→P2c` is a dependent pair.
`P2f` per driver. `P2g` last (composes all). Ship claude-parity feature-by-feature; a driver is "done" when
its normalizer round-trips captured fixtures into the same `marsad_events` claude produces, so the board /
graph / gates / session view work identically regardless of driver.

## Fidelity guardrails (avoid the gaps)
- **Capture real fixtures.** Each normalizer (P2c) is tested against **recorded** protocol transcripts
  from the actual CLI, not hand-invented JSON — the same rigor as `claude_argv` being verified against the
  real binary. Do NOT implement a normalizer from docs alone.
- **Approvals must route to the gate.** Codex approval requests are HITL — they
  map to the existing gate pipeline (and, via Wasl, the three-tier surface). A driver that silently
  auto-approves is a regression.
- **No half-wired driver.** `LaunchSpec.driver` (P2a) may land alone (pure, harmless), but a driver is only
  offered in the UI once P2b–P2g for it are green — otherwise selecting it would spawn a dead session.

## Sources (driver protocols)
- Codex app-server: https://developers.openai.com/codex/app-server and
  https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md
- Codex CLI reference: https://developers.openai.com/codex/cli/reference
