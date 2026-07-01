/**
 * TS view types mirroring the Rust `marsad-viewmodel` board/graph structs
 * (see docs/specs/ui-board.md §Types and docs/specs/viewmodel-ext.md).
 *
 * These mirror the serde JSON shape exactly: snake_case field names,
 * lowercase enum strings. Board types are consumed by `screens/Board.tsx`;
 * the graph/inspector types are declared here (read-only) so later UI
 * blocks (Level 2 — Workstream Graph) can import them without redefining.
 */

// ---------------------------------------------------------------------------
// Shared primitives
// ---------------------------------------------------------------------------

export type HeadState = "needs" | "error" | "working" | "done";
export type Driver = "claude" | "codex" | "gemini";

export interface StateCounts {
  working: number;
  needs: number;
  error: number;
  done: number;
}

// ---------------------------------------------------------------------------
// Board (Level 1)
// ---------------------------------------------------------------------------

/** status: "connected" | "connecting" | "reconnecting" | "unreachable" */
export interface HostChip {
  id: string;
  label: string;
  kind: string;
  status: string;
}

/** status: "degraded" | "down" */
export interface LinkChip {
  a: string;
  b: string;
  status: string;
  note: string | null;
}

export interface BoardSummary {
  workstreams: number;
  sessions: number;
  hosts: number;
}

export interface WorkstreamCard {
  id: string;
  name: string;
  goal: string;
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

// ---------------------------------------------------------------------------
// Graph (Level 2) — mirrors docs/specs/viewmodel-ext.md §Graph
// ---------------------------------------------------------------------------

/** Session phase as surfaced by the graph node (mirrors marsad-workstream::Phase). */
export type Phase =
  | "Needs"
  | "Working"
  | "Error"
  | "Done"
  | "Starting"
  | "Stale"
  | "Detached";

/** Connection kind (mirrors marsad-workstream::ConnKind). */
export type ConnKind = "data" | "command" | "both";

export interface GraphNode {
  id: string;
  name: string;
  driver: Driver;
  host: string;
  state: Phase;
  status: string;
  context_pct: number;
  is_lead: boolean;
  depth: number;
}

export type EdgeStatus = "normal" | "consulting" | "needs";

export interface GraphEdge {
  id: string;
  from: string;
  to: string;
  kind: ConnKind;
  bi: boolean;
  status: EdgeStatus;
  has_gate: boolean;
}

export interface HandoffSummary {
  conn_id: string;
  from: string;
  to: string;
  payload: string;
  risk: string;
}

export interface Inspector {
  working: number;
  needs: number;
  pending_handoff: HandoffSummary | null;
}

export interface GraphView {
  ws_id: string;
  name: string;
  goal: string;
  nodes: GraphNode[];
  edges: GraphEdge[];
  inspector: Inspector;
}
