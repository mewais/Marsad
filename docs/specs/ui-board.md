# U1 — Desktop Board screen (Level 1) — spec (additive, `apps/desktop`)

**Additive UI block.** Do NOT modify existing screens/components/tests. Add a new prop-driven Board
screen + the new TS view types (mirroring the frozen Rust `marsad-viewmodel` structs) + a mock fixture +
Vitest tests. Stack: React + TypeScript, tested with Vitest + `@testing-library/react` (mirror
`tests/Overview.test.tsx`). No Tauri runtime needed — the component takes its data via props.

Design source of truth (port visuals/layout exactly): `docs/design/DESIGN_HANDOFF.md` §"LEVEL 1 —
Workstreams Board" and `docs/design/Marsad Workstreams.dc.html`. Theme tokens exist in
`src/theme/tokens.ts` (`--panel`, `--panel2`, `--border`, `--txt`/`--txt2`/`--txt3`, `--accent`,
`--needs`/`--needs-bg`, `--error`/`--error-bg`, `--done`/`--done-bg`, `--working-bg`, `--shadow`,
`--shadow-sm`, `--font-mono`). Driver colors: claude `#d97757`, codex `#3fb6a8`, opencode `#f0a050`.

## Files this block creates (ONLY these)
- `src/bridge/workstreamTypes.ts` — the new TS view types (see §Types). These mirror the Rust serde
  output exactly (snake_case field names, lowercase enum strings). Board + graph types both live here so
  later UI blocks import (read-only) rather than redefining.
- `src/screens/Board.tsx` — the Board screen component.
- `src/bridge/mockBoard.ts` — `export const MOCK_BOARD: BoardView` used by tests + dev.
- `tests/Board.test.tsx` — Vitest behavioral tests.

## Types (`workstreamTypes.ts`) — mirror marsad-viewmodel exactly
```ts
export type HeadState = "needs" | "error" | "working" | "done";
export type Driver = "claude" | "codex" | "opencode";
export interface StateCounts { working: number; needs: number; error: number; done: number }
export interface HostChip { id: string; label: string; kind: string; status: string } // status: connected|connecting|reconnecting|unreachable
export interface LinkChip { a: string; b: string; status: string; note: string | null }  // status: degraded|down
export interface BoardSummary { workstreams: number; sessions: number; hosts: number }
export interface WorkstreamCard {
  id: string; name: string; goal: string;
  head_state: HeadState | null;
  driver_dots: Driver[];
  host_chips: string[];
  session_count: number;
  counts: StateCounts;
  needs_attention: boolean;
  is_single_node: boolean;
}
export interface BoardView {
  summary: BoardSummary;
  host_chips: HostChip[];
  link_chips: LinkChip[];
  attention: WorkstreamCard[];
  all: WorkstreamCard[];
}
// (also declare the GraphView/GraphNode/GraphEdge/EdgeStatus/Inspector/HandoffSummary types here,
//  mirroring viewmodel-ext.md, so the later Graph block imports them — see docs/specs/viewmodel-ext.md.)
```

## Component: `Board.tsx`
```ts
export function Board(props: { board: BoardView; onOpenWorkstream: (id: string) => void }): JSX.Element
```
Renders, top-to-bottom (per design §Level 1), with these **stable `data-testid`s** for tests:
- `board-title` — the `<h1>` "Workstreams".
- `board-summary` — muted summary line; text MUST contain `"{N} workstreams"`, `"{M} sessions"`,
  `"{K} hosts"` from `board.summary`.
- `host-strip` container; within it one `host-chip-{id}` per `board.host_chips` (text contains the host
  `label` and a status indicator; the chip carries `data-status={status}`), and one `link-chip-{a}-{b}`
  per `board.link_chips` (only degraded/down links are present).
- `attention-section` — rendered ONLY if `board.attention.length > 0`; contains one
  `attention-card-{id}` per attention card. Each card shows the workstream `name`, `goal`, a state pill
  (`data-head-state={head_state}`), host chips, and driver dots (one `driver-dot` per distinct driver,
  each `data-driver={driver}`). Clicking the card calls `onOpenWorkstream(id)`.
- `all-section` — always rendered; one `workstream-row-{id}` per `board.all` card, showing name, goal,
  host chips, driver-dot cluster, `session_count` ("{n} sessions"), a status-pill cluster derived from
  `counts` (e.g. "3 working", "1 needs you" — only non-zero buckets), and a head-state pill
  (`data-head-state`). Clicking a row calls `onOpenWorkstream(id)`.
- A working/needs state dot pulses (class or `data-pulse="true"`) when `head_state` is `working`/`needs`.

Pure presentational: no `useVm`, no Tauri, no network. All data from `props.board`; navigation via
`props.onOpenWorkstream`.

## Mock fixture (`mockBoard.ts`)
`MOCK_BOARD` must include: summary (e.g. 3 workstreams · 6 sessions · 3 hosts); ≥3 host_chips covering
statuses connected + reconnecting; ≥1 link_chip (degraded); ≥1 attention card (head_state `needs`) and
≥1 (head_state `error`); several `all` cards including a single-node one and a multi-driver one.

## Test plan (`tests/Board.test.tsx`, Vitest + testing-library; mirror Overview.test.tsx). Every test a
real `it(...)` with assertions; NO `.skip`/`.todo`/`.only`, no empty tests, no tautologies.
- UB1  summary line shows the workstream/session/host counts from `MOCK_BOARD.summary`.
- UB2  one host chip per `host_chips`, each showing its label and carrying `data-status`; a reconnecting
       host chip has `data-status="reconnecting"`.
- UB3  link chips: a degraded link renders `link-chip-a-b`; NO link chip is rendered for hosts with no
       degraded/down link (assert only the degraded ones present).
- UB4  attention section renders exactly the `attention` cards (by testid); each shows name+goal and a
       `data-head-state` pill; the needs card is `data-head-state="needs"`, the error one `"error"`.
- UB5  attention section is ABSENT when `attention` is empty (render a board with `attention: []`).
- UB6  all-section renders one row per `all` card; a row shows `session_count` ("N sessions") and only
       the non-zero status pills from `counts`.
- UB7  driver dots: a multi-driver card renders one `driver-dot` per distinct driver with the right
       `data-driver`; order matches `driver_dots`.
- UB8  clicking an attention card calls `onOpenWorkstream` with that id; clicking an all-row calls it too
       (use a vi.fn() spy).
- UB9  a `working`/`needs` head-state row/card marks its state dot as pulsing (`data-pulse="true"`); a
       `done` one does not.

(Playwright e2e for full board→graph→session navigation is a LATER block once nav is wired; this block is
Vitest-only and self-contained.)
