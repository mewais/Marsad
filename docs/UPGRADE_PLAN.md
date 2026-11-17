# Marsad → Workstreams: Upgrade Plan (for discussion)

**Verdict: extend, don't rewrite.** The existing codebase is clean, block-decomposed, trait-mockable,
and already enforces the exact test-first / 100%-coverage / anti-cheat regime the upgrade needs.
The durable-session core (detached supervisor, tmux reconnect, async event bridge) is done and
tested. Everything the Workstreams design adds is *additive* and lands on clean existing seams.

## Locked decisions (from this session)
1. **Wasl is an independent MCP server** — usable by anyone, no Marsad dependency. It gates its own
   sends (permission-before-send) and observes/records nothing on Marsad's behalf by default.
2. **Three-tier gating in Wasl** (it's ours, so we make the gate trustworthy and universal):
   - **Tier 1 — MCP elicitation** (clean in-band path): used when the calling client advertises
     `capabilities.elicitation` (Claude Code ✓, Codex ✓). Never carries secrets (spec rule).
   - **Tier 2 — self-owned `prepare`→`commit` handshake** (universal fallback): for clients without
     elicitation (some CLIs lack it) and whenever a real human gate must be
     guaranteed (elicitation can be silently auto-answered by client auto-approve policies).
   - **Tier 3 — optional direct-to-Marsad connector**: when Wasl is configured with Marsad present, it
     surfaces the gate straight to Marsad (reliable regardless of client elicitation support) AND emits an
     **observation feed** so Marsad's graph tracks **every** connection, *including auto-approved ones*.
     Off when Marsad isn't there. This is what makes "someone can use Wasl standalone; Marsad keeps track
     when present" both true.
3. **Wasl capabilities:** consult, prompt/command, share-data; **within one host and across hosts**; and
   **multicast/broadcast** (one→many receivers). Protocol-agnostic internally — requirement is the
   capability, not a specific wire format. Direct host-A→host-B transfer (scp -3/rsync), not routed-through-local.
4. **Wasl topology:** a **per-host Wasl daemon mesh** — each host runs a daemon; daemons peer; local send =
   intra-daemon, cross-host = daemon→daemon, broadcast = fan-out. Keeps Wasl independent of Marsad's SSH.
5. **Both drivers (Claude/Codex) in the driver milestone.** No TUI scraping — each uses its
   bidirectional embedding protocol: Claude control-protocol, Codex `app-server`.
6. **Profiles ≠ Workstreams.** *Profile* (renamed from Workspace) = a named lens grouping workstreams
   (work/personal); pure organization. *Workstream* = a possibly-complex session: 1 agent/1 node (fast
   lane) → N agents/N nodes wired with connections. *Session/Node* = one agent on one host.
7. **A2A protocol:** MCP-only for v1; A2A is a later interop bonus, not required.
8. **Graph layout:** whatever renders cleanly — layered, **no edges routed over nodes, minimized crossings**,
   spline routing. `elkjs` (design's reference) meets this bar; success is measured by the quality bar, not the lib.
9. **Phased delivery**, each phase fully green (100% coverage, clippy clean, e2e passing) before the next.

## Delivery phases
- **Phase 1 — Workstreams core + graph UI (Claude-only, single driver behind the seam).**
- **Phase 2 — Driver trait + Codex driver.**
- **Phase 3 — Wasl A2A/MCP connection layer (live connections, consult/command/transfer, real gating).**

The full block DAG below is delivered up front; the phases only set build/merge order so you can
course-correct between them.

---

# Block map

Legend — **N** = new crate/package, **X** = extend existing. Tier: L0 pure, L1 trait-mocked,
L2 integration-bound, UI component/interaction.

## Phase 1 — Workstreams core + graph

### Rust core

**B14 `marsad-workstream` (N, L0)** — the workstream/graph domain.
- Types: `Workstream{id,name,goal,profile?}`, `Node`(session membership within a ws),
  `Connection{id,ws,from,to,kind:Data|Command|Both,dir?:Bi,gate?}`, `Profile{id,name}` (grouping lens),
  `HeadState`.
- Pure derivations: `head_state(ws)` (needs>error>working>done), `driver_dots(ws)`, `host_chips(ws)`,
  `is_single_node(ws)` (fast-lane detector → open session, skip graph), connection validity/acyclicity checks.
- Lifetime: pure, stateless. No IO. 100% unit-covered.
- Profile is org-only; a workstream belongs to 0..1 profiles. A single-node workstream is the atom the
  old flat "session" becomes.

**B03 `marsad-gates` (X)** — add `Gate::Handoff{from,to,payload,risk,state}` (a gate on a *connection*,
not a session). Extend routing so a decision resolves to a connection `(from,to)` pair; keep single-session
gate routing intact. Idempotent connection-scoped decision IDs.

**B01 `marsad-proto` (X)** — IPC gains workstream/connection scope: `CreateWorkstream`, `AddNode`,
`AddConnection`, `RemoveConnection`, `AnswerHandoffGate`. `LaunchSpec` gains `workstream_id` and a
`driver` field (defaulted to Claude in P1). No wire-format changes to the Claude stream types yet.

**B06 `marsad-store` (X)** — persist `Workstream` + `Connection` + `Profile` entities; snapshot/restore a
whole workstream graph; `.marsad.json` export/import at workstream granularity. **Migration:** each legacy
`Workspace` (bag of session_ids) → a `Profile`; each of its sessions → a single-node `Workstream` in that
profile. `Backend` trait unchanged.

**B11 `marsad-viewmodel` (X)** — the "renderVals port" for the two new levels:
- `board_view(snapshot)` → attention cards, host-health strip, all-workstreams rows, head-state,
  driver dots, status pills, counts summary, and the **profile switcher/filter** (scopes the board).
- `graph_view(ws, snapshot)` → semantic graph: nodes (state/driver/host/status), connections
  (kind/status/gate ref), inspector stats (working/needs tiles, pending-handoff callout, sessions list).
- Note: **layout/geometry is NOT here** — this crate emits the *semantic* graph; the UI lays it out.

**B12 `marsad-core` (X)** — add the workstream layer above sessions: own workstreams + membership +
connection state; route connection/handoff gates; emit new `VmUpdate::{Workstream, Connection,
HandoffGate}` variants. Extend reconnect so relaunch re-adopts *all* sessions of a workstream as a unit.

**B13 `marsad-agent` (X)** — tag each supervised session with its workstream id; emit connection-relevant
events. (Already supervises N sessions, so this is mostly metadata in P1.)

### Desktop UI (split fine, per requirement)

**U1 Board screen (Level 1)** — workstream cards, host-health strip, needs-attention section,
all-workstreams list. Interface: consumes `board_view`. Tests: Vitest (render/state) + Playwright
(triage, open-workstream nav).

**U2a Graph layout module (pure, N)** — deterministic geometry: **By-host** swimlanes (host=column,
depth=row) hand-computed; **Flow** via `elkjs` (layered DAG, spline routing). Edge geometry: bottom→top
stubs, arrowheads (both ends for consulting), dash-encoding (data=dashed, command=dotted, both=solid),
gate-diamond midpoints. Pure functions over the semantic graph. Tests: Vitest, exhaustive (positions,
routing, anchoring, gate placement) — this is the highest-value automatable UI logic.

**U2b Graph canvas + interaction** — pan/zoom, node render, SVG edges, gate diamonds, layout toggle,
zoom controls, legend, neighbourhood highlight (dim others to 0.34), click→session. Consumes U2a.
Tests: Playwright.

**U3 Session view (Level 3, mostly X of existing SessionDetail)** — add driver pill, breadcrumb
(Marsad / Workstream / Session), gate banner, **HITL collapse-to-summary** (question/plan/permission →
resolved `▸` rows), background-jobs tray. Heavy reuse of existing tabs (Activity/Changes/Usage/Config/
Terminal) and components (DiffView, Composer, ToolUse). Tests: Vitest + Playwright.

**U4 Design mode** — authoring canvas: `＋ Add agent` (node modal), drag-to-connect (rubber-band,
zoom-accurate), inline controller/feeder wiring, edge-type choice, Confirm & launch / Discard. Tests:
Playwright (drag/connect/launch flow).

**U5 Modals & overlays** — New-workstream (Single-session fast lane vs Build-it-out), node modal
(project dir + host + model + posture + first prompt + **driver picker** + inline wire pickers), HITL
overlays (batched questions / plan / permission), **cross-host handoff gate modal** (source→dest, payload,
risk, approve/reject), Settings (Hosts w/ per-driver agent commands; Session defaults w/ default driver;
Notifications). Tests: Vitest + Playwright.

**U6 Nav + state slice (X of `useVm`)** — screen adds `board`/`graph`; new reducer slices for
workstream / graph / node-selection / design-draft; **event batching/coalescing** at the
bridge→store boundary (the fix for the "buffers on every little thing" complaint — collapse bursts of
`VmUpdate` into a single render frame; memoize selectors). Tests: Vitest (reducer + coalescing timing).

**U7 Bridge api + types (X)** — extend `types.ts` (Workstream, Node, Connection, Driver, `Gate::Handoff`),
`api.ts` (workstream/connection CRUD, launch-with-driver, answer-handoff-gate), new `VmUpdate` variants.
Tests: Vitest against the mock fixture.

**D01 src-tauri bridge (X)** — Tauri commands mirroring the new core API; emit the new `VmUpdate`
variants. Lock-drop-before-await discipline preserved. Tests: Rust unit over `marsad-core` fakes.

## Phase 2 — Driver trait + Codex

**B16 `marsad-driver` (N, L1)** — the `Driver` trait:
`spawn_argv(spec)`, `parse(native_line) -> Vec<Event>`, `encode(action) -> native_line`,
`approval_channel` mapping (native approval req → `marsad_gates::Gate`, and decision → native reply),
`model_catalog()`, `capabilities()`, `posture_map(Posture)`.
- Impls: `ClaudeDriver` (extracts today's proto/events logic), `CodexDriver` (`app-server` JSON-RPC-lite). Each verified against recorded-real fixtures captured from the real CLI.
- `marsad-proto` (X): split per-driver wire types into driver submodules; keep shared GUI↔agent IPC.
- `marsad-events` (X): reducer now consumes the driver-neutral `Event`; subagent/gate detection
  parameterized (drop the hardcoded `"Task"` tool assumption).

**B13 `marsad-agent` (X)** — spawn via `Driver::spawn_argv`, route native lines through `Driver::parse`,
intercept approvals via the driver's channel. tmux/transport/FIFO scaffolding unchanged (driver-neutral
already). Recorded-real + live (CI-gated) tests per driver.

**B05/B10/B11 (X)** — driver-aware coloring (format), driver-aware notification kinds (notify),
per-driver model pickers + normalized posture (viewmodel).

**UI (X)** — driver picker in node modal; driver color-coding everywhere; per-driver model catalogs;
per-host **per-driver agent-command overrides** in Settings→Hosts (matches the design's host `agents[]`).

## Phase 3 — Wasl (agent-to-agent connection layer)

Repo: `Wasl` (currently empty). **Wasl is a fully independent MCP server**; the Marsad connector is optional.

**W1 MCP surface + tool schema (N)** — stdio + HTTP transports; tools `consult(peers,q)`,
`command(peers,task)`, `message(peers,payload)`, `transfer(src_host,dst_host,path)`. `peers` accepts
one or many → **multicast/broadcast**. Each mutating call runs the gate (W5) before acting.
**W2 Peer registry / identity + daemon mesh (N)** — agent cards (who/which host/which workstream);
discovery; the **per-host daemon mesh** (each host runs a Wasl daemon; daemons peer; local = intra-daemon,
cross-host = daemon→daemon, broadcast = fan-out). Independent of Marsad's SSH.
**W3 Routing (consult/command/message) (N)** — mailbox/inbox; deliver to each target's client as an
injected prompt/message; reply + fan-in for multicast. (Borrow mcp_agent_mail's Git+SQLite audit design.)
**W4 Transfer tool (N)** — `scp -3`/`rsync` **direct host-A→host-B** (and A→many), as a structured
gateable call.
**W5 Three-tier gate (N)** — the permission-before-send engine: Tier 1 MCP `elicitation/create` when the
client supports it; Tier 2 self-owned `prepare`→`commit` handshake with Wasl's own approval channel
(covers any non-elicitation client, and guarantees a real human gate); Tier 3 **optional
direct-to-Marsad connector** — surfaces the gate to Marsad and emits an **observation feed for every
connection incl. auto-approved** so Marsad's graph stays complete. Feature-detects per client; degrades
gracefully. This replaces the old "forward-everything-to-Marsad brain" design — Marsad coupling is now
optional, not load-bearing.
**W6 Audit store (N)** — durable, human-auditable log of every interaction + decision (works standalone).

**B12 core / B13 agent (X)** — Marsad consumes the W5 Tier-3 feed: render live `Connection`s (incl.
auto-approved), drive gate approvals surfaced by Wasl, resolve decisions back. **B03 gates (X)** — the P1
handoff/consult/command gate types now also accept Wasl-originated gates. **UI (X)** — connections become
live (not just design-authored); consult/command/transfer render as typed edges + gate diamonds; the
handoff modal drives real approvals; broadcast renders as one→many edges.

---

# Integration / end-to-end test ladder (per your steps 6 & 8)
Block tests → pair tests (e.g. driver↔agent, core↔store, viewmodel↔UI) → level tests (board, graph,
session) → cross-level e2e (author a workstream in design mode → launch → gate a cross-host handoff →
observe on the board) → multi-driver e2e → Wasl consult/command/transfer e2e with a real HITL gate.
Everything automatable is tested; only strictly-visual pixel fidelity is left to manual review.

# Resolved (this session)
- Workspace → renamed **Profile** (grouping lens over workstreams), kept + migrated. Workstream is the
  new unit (complex session). ✔
- Wasl: independent MCP, **per-host daemon mesh**, **multicast/broadcast**, three-tier gate with optional
  Marsad connector (gate-fallback + observation feed incl. auto-approved). ✔
- A2A: MCP-only v1; A2A later. ✔
- Graph layout: clean layered, no edges over nodes, minimized crossings (`elkjs` meets the bar). ✔

# Verify before Phase 3 (known unknown, non-blocking for P1/P2)
- Whether Claude Code forwards MCP elicitation over its **headless control protocol** to a driving parent
  (Marsad). If yes, Tier-1 covers Claude-in-Marsad; if no, Tier-3 Marsad connector covers it. Either way
  the design holds — this only decides which tier Claude-in-Marsad uses.
