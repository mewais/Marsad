# Handoff: Marsad — Workstreams (multi-agent orchestration console)

## Overview
Marsad ("مرصد", *watchtower/observatory*) is a desktop console for supervising many concurrent AI coding-agent sessions running across multiple machines. This handoff covers the **Workstreams** redesign — the current, canonical version of the product.

The central concept is a **workstream**: one *or more* agent sessions, running across one *or more* hosts, optionally wired together into a pipeline. The UI is a three-level zoom:

1. **Level 1 — Board:** status of every workstream at a glance (what needs you, what's running, what errored), plus host connectivity health.
2. **Level 2 — Graph:** a workstream opened as a node-graph of its agent sessions, with typed connections (data / command) between them, host swimlanes, and human-in-the-loop (HITL) gates sitting on the wires.
3. **Level 3 — Session:** a single agent session — transcript, composer, changes/diff, usage/cost, config, terminal.

It also has a **design mode** for authoring a new multi-agent workstream (place agents, wire them, then "Confirm & launch"), distinct from the **operate mode** used to monitor a running one.

There are three "layers" the product reasons about, worth understanding before implementing:
- **Frontends:** desktop (this file), mobile (separate).
- **Backends / hosts:** where a session runs — `local`, `WSL`, or `remote` (SSH).
- **Drivers:** which agent CLI powers a session — **Claude Code**, **Codex**, or **opencode**. Drivers are first-class and colour-coded everywhere.

## About the Design Files
The file in this bundle (`Marsad Workstreams.dc.html`) is a **design reference created in HTML** — a working, interactive prototype that demonstrates the intended look, layout, states, and behaviour. **It is not production code to ship or copy directly.**

It is authored as a "Design Component" (a single self-contained HTML file with an inline template + a `Component` logic class). That authoring format is an artifact of the prototyping environment — **do not** try to preserve it. Your task is to **recreate these designs in the target codebase's existing environment** (React, Vue, Svelte, SwiftUI, native, etc.) using its established component patterns, state management, and styling system. If there is no existing frontend yet, choose the most appropriate framework for the project and implement there.

Treat the HTML as the source of truth for *visual design and interaction*, and this README as the source of truth for *structure, tokens, and behaviour*.

## Fidelity
**High-fidelity (hifi).** Final colours, typography, spacing, and interactions. Recreate the UI pixel-accurately using the codebase's libraries. All hex values, sizes, and radii below are exact. Note the design is a **dark-first** theme with a full light theme; both palettes are specified under Design Tokens.

## Tech notes / dependencies
- **Graph auto-layout:** the graph (Level 2) uses **[elkjs](https://github.com/kieler/elkjs)** (Eclipse Layout Kernel, `elk.bundled.js`) for the **Flow** layout (layered DAG with spline edge routing). The **By host** layout is computed by hand (host = column/swimlane, dependency depth = row). Use `elkjs` (or a comparable layered-graph layout engine such as dagre) in the real implementation for Flow mode. Edge geometry (S-curves, boundary anchoring, arrowheads) is custom SVG — see **Graph rendering** below.
- **Markdown rendering:** agent responses render a small subset of Markdown (headings, bold, italic, inline code, fenced code blocks, lists). Use the codebase's existing Markdown renderer + syntax highlighter.
- **Fonts:** Hanken Grotesk (UI) and JetBrains Mono (code/paths/metrics), loaded from Google Fonts.
- No other runtime dependencies; all layout is fl/grid + absolute-positioned SVG for the graph.

---

## Screens / Views

### LEVEL 1 — Workstreams Board (`view: 'board'`)
**Purpose:** the home screen. See the status of all workstreams and host health; triage what needs attention; open a workstream or start a new one.

**Layout:** vertical scroll, max content width **1200px**, centered, padding `22px 26px 40px`. Top-to-bottom:
1. **Title row:** `<h1>` "Workstreams" (21px/700, letter-spacing -0.3px) + a muted summary line ("N workstreams · M sessions · K hosts", 12.5px, `--txt2`).
2. **Sub-line** (12.5px `--txt3`): "A workstream is one or more sessions across one or more hosts. Open one to inspect its graph, or jump straight into a session."
3. **Host health strip** (see below).
4. **"Needs your attention" section** (only if any workstream is gated/errored): a pulsing amber dot + uppercase heading, then a responsive grid (`repeat(auto-fill, minmax(380px, 1fr))`, gap 13px) of attention cards.
5. **"All workstreams" section:** uppercase heading, then a vertical list (gap 9px) of workstream rows.

**Host health strip:** a horizontal wrap (gap 9px). Leading uppercase "Hosts" label, then one chip per host and one chip per degraded host-to-host link.
- **Host chip:** `padding 5px 11px`, radius 8px, `background var(--panel)`, `border 1px solid` (border colour = `--border` when connected, else the status colour). Contents: status dot (8px circle) + mono host label (`▪ localhost` / `⊞ wsl…` / `⤓ hostname`) + status text.
  - Status → colour + text: `connected` = `--done` "connected" (no animation); `reconnecting` = `--needs` "reconnecting" (dot pulses, `dotpulse 1s`); `down` = `--error` "unreachable" (dot pulses `0.8s`).
- **Link-issue chip:** `background var(--needs-bg)`, `border 1px solid var(--needs)`. Contents: `⚠` + mono text "hostA ⇄ hostB link degraded" + a muted note ("packet loss · 38%").

**Attention card** (clickable → opens workstream):
- Radius 14px, padding 16px, `background var(--panel)`, `border 1px solid <stateColor>`, `box-shadow 0 0 22px <glow>` (glow = amber-tint for needs, red-tint for error). A 3px full-height accent bar (`<stateColor>`) on the left edge (absolute).
- Header row: a small uppercase state pill (`color/background` = state colour + tint) on the left; on the right, mono host chips (`▪ localhost` etc.).
- Workstream **name** (15.5px/700, -0.2px) + **goal** (11.5px `--txt3`).
- A "headline" info box (`margin 12px 0`, padding `10px 12px`, radius 9px, `background var(--panel2)`, `border 1px solid var(--border)`, 12px text) describing the specific blocker (e.g. "Cross-host handoff waiting — build artifact → test host.", or "Refactor auth middleware wants to run: npx prisma migrate deploy").
- Footer row: a primary CTA button filling the row (`background <stateColor>`, `color #0e1116`, 700, radius 9px, padding 9px) labelled contextually ("Review handoff" / "Review" / "Open"), then a cluster of session status dots (9px circles, working ones pulse).

**Workstream row** (clickable → opens workstream), `display:flex; align-items:center; gap:16px; padding:14px 16px; radius:13px; background:var(--panel); border:1px solid var(--border)`:
- Leading state dot (10px; pulses if the workstream's head-state is "working").
- Name+goal block (min 210 / max 280px): name 13.5px/600 (ellipsis), goal 11px `--txt3` (ellipsis).
- Host chips (fixed 150px): mono chips `▪ localhost` in `--panel2` pills.
- Driver dots + session count (fixed 88px): tiny 7px dots tinted per driver, then "N sessions".
- Status pills (flex 1, wrap): one pill per state present, e.g. "3 working", "1 needs you" (`color/bg` = state colour + tint).
- Head-state pill (uppercase) + a `›` chevron.

**Head-state** of a workstream = the most urgent state among its sessions, priority order: `needs` > `error` > `working` > `done`.

---

### LEVEL 2 — Workstream Graph (`view: 'graph'`)
**Purpose:** inspect one workstream as a graph of its agent sessions and the connections between them; spot the HITL gate; drill into a session. Also hosts **design mode** (authoring).

**Layout:** two columns — a graph **canvas** (flex 1) and a right **inspector** (width 320px, `border-left`, translucent `rgba(18,22,29,0.74)` + `backdrop-filter: blur(22px)`).

**Canvas:** an absolutely-positioned pannable/zoomable surface inside a scroll container. It has a dotted grid background (`radial-gradient(circle, rgba(255,255,255,0.04) 1px, transparent 1px)`, size 26px). A zoom wrapper applies `transform: scale(z)` (transform-origin top-left) and a spacer div sizes the scroll area to `canvasW*z × canvasH*z`; the content is centered horizontally when smaller than the viewport.

**Two layout modes** (toggle, bottom-left):
- **By host** (default): each host is a **swimlane column** (width 222px, gap 30px). A translucent zone box per host (radius 16px, `background rgba(21,25,33,0.5)`, `backdrop-filter: blur(10px)`, header with host icon/label/kind pill/status dot). Within a lane, nodes stack by **dependency depth** (row height 132px). This keeps "where does it run" instantly legible.
- **Flow:** a compact free layered DAG (elkjs, direction DOWN, spline routing, crossing minimization). No host lanes; instead each node shows a host chip. Graph is centered.

Both modes: **top-to-bottom flow** (dependencies point downward), zoom auto-**fits** on open/layout-change.

**Node** (agent session), width **190px**, height **66px**, `padding 9px 12px`, radius 13px, `background rgba(26,31,41,0.94)`, `border 1px solid var(--border2)`, **left accent border 3px** in the session's state colour, `box-shadow var(--shadow-sm)`:
- Top row: state dot (8px, pulses if working) + name (12.5px/600, ellipsis) + (design mode only) a "LEAD" badge is **not** used — hierarchy is expressed by control edges, not a badge.
- Bottom row: **driver pill** (9.5px/700, `padding 2px 7px`, radius 5px, `color/background/border` = driver colour + tint) + (Flow mode) a host chip + status text (mono, ellipsis) + a `›` chevron.
- Selected node: border becomes the state colour. Dimmed to opacity 0.34 when another node's neighbourhood is highlighted.
- In **design mode**, nodes show small **connect handles** (circles) used to drag connections; these are hidden in operate mode.

**Edges** (SVG paths, `z-index 2`, non-interactive except gate):
- **Direction:** every edge has an arrowhead at its target; **consulting (two-way)** edges have arrowheads at *both* ends.
- **Axis-aligned ends:** each edge leaves the bottom of its source and enters the top of its target with a straight vertical **stub** (length **24px in Flow, 40px in By-host**) so arrowheads always point cleanly up/down and the curve eases into vertical. Interior routing is a smoothed spline (quadratic-midpoint through ELK bend points in Flow; hand-computed side-waypoints that bend around intervening nodes in By-host).
- **Edge semantics — colour = status, dash = type:**
  - Colour: **blue `--accent`** = normal; **grey `--txt2`** = consulting/two-way; **amber `--needs`** = needs attention (has an unresolved gate).
  - Dash pattern encodes type: **Data / message** = dashed (`stroke-dasharray: 8 7`, animated "flow" marching ants); **Command / prompt** = dotted (`stroke-dasharray: 2 5`, round caps); **Both** = solid line. (Do **not** colour-code the type — type is purely dash-encoded; colour is reserved for status.)
- **Gate marker (HITL):** a 30×30 amber diamond (rotated 45° square, `background var(--needs-bg)`, `border 1px solid var(--needs)`, `animation glow`) sits at the midpoint of a gated edge, containing a `▲`. Click → opens the gate/handoff modal. This is the signature visual: *a wire crossing a host boundary with a gate sitting on it.*

**Graph toolbar overlays** (absolute, over the canvas):
- **Edge legend** (top-left): a small translucent card listing the four edge styles (Data/message, Command/prompt, Both, Consulting two-way, Needs attention) each with a mini SVG swatch.
- **Layout toggle** (bottom-left): segmented "By host" / "Flow".
- **Zoom controls** (bottom-right): `−`  `NN%` (click % to reset to 100)  `＋`  `Fit`.

**Graph inspector (operate mode):** workstream name/goal, two stat tiles (working count in blue tile, needs-you count in amber tile), a governed-handoff callout (if a gate is pending) that opens the gate modal, and a **Sessions** list (click a row → Level 3). Also an **"✎ Update workflow"** button that re-enters design mode.

**Graph inspector (design mode):** a **Design** panel — heading "Design mode", copy "Place agents and wire them together. Nothing runs until you confirm.", **Confirm & launch** (green) and **Discard** buttons, then an "AGENTS (N)" list. There is **no** cost/stats in design mode. A **＋ Add agent** button sits above the canvas (design mode only) and opens the node modal.

---

### LEVEL 3 — Session (`view: 'session'`)
**Purpose:** work with a single agent session.

**Header** (padding `15px 24px 0`, bottom border): title (18px/700) + uppercase state pill; a mono meta row — **driver pill** (bordered), host (`▪ localhost`), branch (`⎇ …`), context (`ctx NN%`). A "‹ Back" button (label = parent workstream name if arrived from a multi-session graph, else "Workstreams"). Below: a **tab bar** (Activity / Changes / Usage / Config / Terminal) — active tab has a 2px `--accent` bottom border and `--txt` colour; inactive `--txt3`.

**Activity tab (default):**
- Optional **gate banner** (if the session has a pending gate) pinned under the header: amber, glowing, with `▲` + lead text + mono summary + "Review →".
- **Transcript** (max 760px, centered, gap 14px). Message types:
  - **Prompt** (user): right-aligned blue bubble (`background var(--accent)`, white, radius `13px 13px 4px 13px`, max 78%).
  - **Response** (agent): 26px rounded driver avatar (gradient, driver mark letter) + **Markdown-rendered** body (13px/1.6). Renders headings, bold/italic, inline `code` chips, fenced code blocks (syntax-highlighted), bullet/numbered lists.
  - **Tool call:** collapsible row (`background var(--panel)`, border) — caret + `⚙` + tool name + mono args (ellipsis) + status; expands to a `<pre>` output block.
  - **Bash:** a framed block — `$ command` (mono, accent) over a `--panel2` output `<pre>`.
  - **Background jobs (subagents):** an inline note "Spawned N background job(s) — tracked in the tray below" (the tray is a pinned strip; see below).
  - **Questions (batched HITL):** if pending → an amber glowing card "N question(s) need you" + "Answer →" opening the questions overlay. Once answered → collapses to a `▸`-expandable "Questions — N answered" summary row showing each question, its options, and the picked/custom answer (✓ marks the choice).
  - **Permission (HITL):** if pending → amber card "Permission needed" with a mono `$ command` preview + "Review →" (opens permission overlay) + meta. Once decided → collapses to a `▸` summary row with the command and the decision (✓ approved / • rejected).
  - **Plan mode (HITL):** if pending → amber card "Plan mode — needs you" + "Review plan →" (opens plan overlay) + step count. Once decided → collapses to a `▸`-expandable "Plan" summary showing the numbered steps + the decision.
  - **System:** centered mono pill (rounded), muted.
  - **Error:** red `<pre>` block (`background var(--error-bg)`, `border var(--error)`).
  - **Working:** driver avatar + a spinning ring + status text.
- **Background jobs tray** (pinned above composer, only if jobs exist): a `--panel2` strip; header "⑂ Background jobs" + running/total summary + collapse caret; expands to rows (status dot + name + driver pill + mono host + status).
- **Composer** (pinned bottom): `max 760px`, a rounded `--panel2` input group — a `/` slash-command button (opens a command menu), a growing `<textarea>` (Enter=send, Shift+Enter=newline, **Esc=pause/interrupt**, `/`=slash menu), and a "Send ↵" button. A hint bar shows "⎋ pause" etc.

**Changes tab:** a file list (M/A/D letter + name + `+adds` / `−dels`) alongside a **side-by-side Before/After diff** (two columns, line numbers, add/del line tinting) and a commit bar (message input + Commit button).

**Usage tab:** a context-window bar (turns amber past 70% with a compaction warning), tokens/cost/turns stat tiles, and a **Cost breakdown** — **By host** and **By node**, each with a stacked bar and per-row entries (driver pill, cost, % share, sorted by cost).

**Config tab:** driver-aware **model** picker (Opus/Sonnet/Haiku for Claude Code; GPT-5 variants for Codex), **permission posture** (Ask every time / Auto-approve safe / Full autonomy), and **tools + MCP** toggles.

**Terminal tab:** the raw escape-hatch terminal for the session (mono, dark).

---

### Modals & overlays
All modals: fixed overlay `rgba(5,7,10,0.64)` + `backdrop-filter blur(3px)`, centered, `z-index 60`; card `background var(--panel)`, `border 1px solid var(--border2)`, radius 18px, `box-shadow var(--shadow)`, `animation slideup .18s ease`.

- **New workstream:** name + goal, then a two-path choice — **Single session** (fast lane → opens node modal → Launch → straight to Level 3) or **Build it out** (→ empty design-mode canvas).
- **Node modal (add / configure an agent):** the classic launch form — name, project directory (with recent-dir chips), host picker, model override, permission override, first prompt — **plus** an **Agent/driver picker** (Claude Code / Codex / opencode) and inline **"Controlled by" / "Feeds from"** pickers to wire the new node to existing ones without dragging. First agent added becomes the effective lead.
- **HITL overlays** (question batch / plan / permission): a focused reading window. Questions overlay lists all pending questions together, each with option buttons + a "Write your own…" field, and one "Submit answers". Permission overlay shows the full (multi-line) command with **Approve & run / Approve all this session / Reject**. Plan overlay shows the numbered plan with **Approve / Keep planning**.
- **Cross-host handoff gate:** shows Source→Destination hosts, the payload being transferred, a risk note, and **Approve handoff / Reject**.
- **Settings** (⚙): 880px wide, ~86vh tall, tabbed:
  - **General:** Appearance (Dark/Light), Background (Gradient / Custom image / Desktop), Behaviour toggles (translucency, start-on-login, restore workspace, confirm-before-stop).
  - **Hosts:** a **"This machine"** section (localhost — agent commands, setup command, environment, always shown) + a **"Connected hosts"** list (remote/WSL). Each host card is expandable: per-agent run-command overrides, setup command, env vars, and (remote) the connection target. An **add-host** form: Label + Target (format hints `name` / `ssh:` / `wsl:` / `wslssh:` / `plink:`), per-agent command overrides, optional setup command, optional environment block.
  - **Session defaults:** default driver → default model → permission posture.
  - **Notifications:** needs-you / finished / errored toggles, sound, grouping, Do Not Disturb.

---

## Interactions & Behavior
- **Drill-down / breadcrumbs:** header shows `Marsad / <Workstream> / <Session>`; each crumb navigates. Logo → board. Opening a **single-session** workstream skips the graph and goes straight to its session; a **multi-session** workstream opens the graph.
- **Design vs operate:** design mode = author (add agents, drag/inline-wire, nothing runs) → **Confirm & launch** transitions to operate (live sessions, cost, stats). **Discard** removes the draft. Operate → **Update workflow** re-enters design.
- **Drag-to-connect (design mode):** drag from a node's connect handle to another node; a rubber-band line tracks the cursor (measured against the scaled canvas rect, so it's accurate at any zoom); dropping opens edge-type choice.
- **Graph:** click a node → highlight its neighbourhood (others dim to 0.34) and, in operate mode, open the session; click empty canvas → clear selection; click a gate diamond → gate modal. Zoom via toolbar; auto-fit on open.
- **HITL resolution:** pending question/plan/permission cards are amber + glow; once resolved they collapse into muted `▸`-expandable summary rows in the transcript. This "needs you → resolved" collapse is uniform across all three HITL types.
- **Composer keys:** Enter = send, Shift+Enter = newline, Esc = pause/interrupt (or close slash menu), `/` at line start = slash menu.
- **Theme toggle:** ☀/☾ in header swaps the entire palette instantly (see tokens).
- **Live simulation:** in the prototype, working sessions periodically advance (tokens/context tick, status text changes) and a newly-launched session goes `starting → working`. In production this is driven by real session telemetry.

## Animations & transitions
- `dotpulse` (opacity 1↔0.35): status dots for working/reconnecting states. 1.3s working, 1s reconnecting, 0.8s down.
- `flow` (`stroke-dashoffset` marching): animated data edges, 1.1s linear infinite.
- `glow` (box-shadow pulse in amber): anything that "needs you" — gate diamonds, HITL cards, gate banner. ~2.6–2.8s ease-in-out.
- `spin`: the working-message loading ring, 0.8s linear.
- `slideup` (translateY 8px + fade): modal entrance, 0.18s.
- Edge auto-fit uses an instant transform; no long transitions on the canvas.

## State Management
Top-level UI state (from the prototype's `Component`):
- `view`: `'board' | 'graph' | 'session'`.
- `wsId`, `sessionId`: current workstream / session.
- `fromGraph`: whether the session was opened from a graph (controls the Back label).
- `graphLayout`: `'host' | 'flow'`; `graphZoom` (number); `didFit` (bool).
- `nodeSel`: selected node id in the graph (drives neighbourhood highlight).
- `designMode` / draft workstream: nodes + connections being authored before "Confirm & launch".
- Drag-connect transient state: source node + live cursor point.
- `theme`: `'dark' | 'light'`.
- Modal/overlay flags: new-workstream, node modal, settings (+ active settings tab), and the active HITL overlay (question/plan/permission) + which transcript item it targets.
- Per-session UI: active tab, collapsed/expanded tool rows, background-jobs tray open, resolved-HITL expand state.
- Toast (transient confirmation messages).

**Domain data model** (what a real backend would provide):
- **Host:** `{ id, label, kind: 'local'|'wsl'|'remote', status: 'connected'|'reconnecting'|'down', target?, agentCommands?, setupCommand?, env? }`.
- **Host link:** `{ a, b, status: 'ok'|'degraded'|'down', note? }`.
- **Workstream:** `{ id, name, goal }` (its sessions/connections are derived by `ws` foreign key).
- **Session (node):** `{ id, ws, name, driver: 'claude'|'codex'|'opencode', host, state: 'working'|'needs'|'error'|'done'|'starting', role?, branch, contextPct, status, gate? }`.
- **Connection (edge):** `{ id, ws, from, to, kind: 'data'|'command'|'both', dir?: 'bi', gate? }`.
- **Gate (HITL):** on a session (permission) or on a connection (cross-host handoff): `{ kind, title, command?/payload?, cwd?, risk?, state?: 'pending'|'approved'|... }`.
- **Transcript item:** discriminated union — `prompt | response | tool | bash | subagents | questions | permission | plan | system | error | working`.
- Derived: workstream **head-state** = max-severity session state (`needs>error>working>done`); driver dots/host chips = distinct drivers/hosts among a workstream's sessions.

## Design Tokens

### Colours — Dark theme (default)
| Token | Hex / value | Use |
|---|---|---|
| `--bg` | `#0e1116` | app background |
| `--panel` | `#151921` | cards, rows |
| `--panel2` | `#1a1f29` | insets, chips, composer |
| `--elev` | `#1e2530` | elevated (toolbar cards, menus) |
| `--border` | `#242b37` | default borders |
| `--border2` | `#313a48` | stronger borders |
| `--txt` | `#e8ecf1` | primary text |
| `--txt2` | `#97a1b1` | secondary text; also **consult-edge grey** |
| `--txt3` | `#5f6979` | muted text |
| `--accent` | `#5b8def` | brand / normal edges / working |
| `--accent2` | `#7aa2f5` | accent hover/gradient end |
| `--needs` | `#f1a83a` | needs-you (HITL, attention) |
| `--error` | `#e0594a` | error |
| `--done` | `#46c08a` | done / connected / confirm |
| `--neutral` | `#7e8896` | neutral |
| `--needs-bg` | `rgba(241,168,58,0.13)` | needs tint |
| `--error-bg` | `rgba(224,89,74,0.12)` | error tint |
| `--done-bg` | `rgba(70,192,138,0.12)` | done tint |
| `--work-bg` | `rgba(91,141,239,0.12)` | working tint |
| `--accent-bg` | `rgba(91,141,239,0.14)` | accent tint |

### Driver colours (both themes)
| Driver | Token | Hex |
|---|---|---|
| Claude Code | `--d-claude` | `#d97757` |
| Codex | `--d-codex` | `#3fb6a8` |
| opencode | `--d-opencode` | `#f0a050` |

Driver pills use the driver hex as text + border, over a ~12% tint of it as background.

### Colours — Light theme
`--bg:#eef1f6; --panel:#ffffff; --panel2:#f4f6fa; --elev:#ffffff; --border:#e2e7ee; --border2:#cfd6e0; --txt:#1a222e; --txt2:#5a6675; --txt3:#8a94a3; --needs:#c07d10; --error:#cf4436; --done:#1f9d6b; --needs-bg:rgba(192,125,16,0.12); --error-bg:rgba(207,68,54,0.10); --done-bg:rgba(31,157,107,0.10); --work-bg:rgba(63,111,214,0.10); --accent-bg:rgba(63,111,214,0.10)` (accent stays `#5b8def`; shadows lighten to `rgba(20,30,50,…)`).

### Shadows
- `--shadow`: `0 14px 44px rgba(0,0,0,0.5)` (dark) / `0 14px 44px rgba(20,30,50,0.16)` (light) — modals, menus.
- `--shadow-sm`: `0 2px 10px rgba(0,0,0,0.32)` (dark) / `0 2px 10px rgba(20,30,50,0.09)` (light) — nodes, small cards.

### Typography
- **UI:** Hanken Grotesk. Weights used: 400/500/600/700/800.
  - H1 21px/700 (-0.3px); screen titles 18px/700 (-0.2px); section headings 13px/700 uppercase (letter-spacing .4px); body 13–13.5px; labels 10–11px/700 uppercase (letter-spacing .4–.6px); micro 9–9.5px.
- **Mono:** JetBrains Mono — all paths, hosts, commands, metrics, diffs, ids. 9.5–12px.

### Spacing & shape
- Base gaps: 6 / 8 / 9 / 12 / 14 / 16px. Screen padding 22–26px; card padding 13–16px.
- Radii: chips/pills 5–8px; buttons 8–9px; cards 11–14px; modals 18px; status dots fully round.
- **Graph metrics:** node 190×66px; By-host lane width 222px, gap 30px, row height 132px; edge stub 24px (Flow) / 40px (By-host); grid dot 26px; gate diamond 30×30.
- **Translucency (glass):** chrome surfaces (header, sidebars, inspector, zones) use `rgba(...,0.5–0.74)` fills + `backdrop-filter: blur(10–22px)`. A soft multi-radial-gradient wallpaper sits behind everything (blue/violet/teal blooms).

### Iconography
Simple text/unicode glyphs are used as placeholders: host kinds `▪` (local) `⊞` (WSL) `⤓` (remote); `⎇` branch; `⚙` tool; `⑂` background jobs/subagents; `▲` needs-attention; `❑` plan; `�A/Cx/G` driver marks; `›` chevron; `☀/☾` theme; `⚠` warning. Replace with the codebase's icon set.

## Assets
No image assets. Fonts via Google Fonts (Hanken Grotesk, JetBrains Mono). Graph layout via `elkjs` (`elk.bundled.js`). The wallpaper and all iconography are CSS/unicode — no files to extract.

## Files
- `Marsad Workstreams.dc.html` — the complete interactive design reference (all three levels, both graph layouts, design/operate modes, all modals, both themes). Open it in a browser to explore every state.

### How to reach each state in the prototype (for reviewing behaviour)
- **Level 2 graph with everything:** open **"Release v2.4"** from the board — 5 sessions across 3 hosts, control + data + consult edges, and the cross-host handoff gate diamond.
- **Rich Level 3 transcript:** open the **"Release orchestrator"** session — collapsible tools, bash, background-jobs tray, batched questions, plan-mode card, and a permission gate.
- **Permission gate + real diff:** the **"Refactor auth middleware"** session (Changes tab has the side-by-side diff).
- **Design mode:** header **＋ New workstream → Build it out** → empty canvas → **＋ Add agent**.
- **Host health / link degradation:** the board's Hosts strip (gpu-box reconnecting; hetzner ⇄ gpu-box link degraded).
