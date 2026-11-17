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

export type HeadState = "needs" | "error" | "working" | "done" | "paused";
export type Driver = "claude" | "codex" | "opencode" | "wasl-claude" | "wasl-opencode" | "wasl-codex";

export interface StateCounts {
  working: number;
  needs: number;
  error: number;
  done: number;
  paused: number;
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
  /**
   * Session ids in this card (node order). The dashboard looks up each session's
   * canonical `SessionView` from `state.sessions` to derive per-session status
   * (badge / connection hint / resume-pause) from the SAME source of truth as the
   * session-detail view — never a divergent aggregate. See bridge/sessionStatus.ts.
   * Optional for back-compat with older snapshots / mock boards.
   */
  node_ids?: string[];
  counts: StateCounts;
  needs_attention: boolean;
  is_single_node: boolean;
  /**
   * Optional — the attention card's "headline" blocker description (design
   * §Level 1 Attention card, e.g. "Cross-host handoff waiting — build
   * artifact → test host."). Additive/optional so a real backend that
   * doesn't populate it yet degrades gracefully (Board falls back to
   * `goal`). Populated by the mock bridge for attention cards.
   */
  headline?: string;
  /**
   * Optional — contextual CTA button label for the attention card
   * ("Review handoff" / "Review" / "Open"). Additive/optional; Board falls
   * back to a head-state-derived default when absent.
   */
  cta_label?: string;
  /** A noteworthy end reason to surface as a card chip (a real error's reason,
   *  or a user Cancel). Absent/null when nothing noteworthy. */
  end_reason?: EndReason | null;
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

/** Why a node's last turn ended (mirrors marsad_events::EndReason). Duplicated
 *  here to keep this view-types module self-contained, like `Phase`/`Driver`. */
export type EndReason = "Cancelled" | "MaxTurns" | "ExecutionError";

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
  /** Noteworthy end reason for a per-node reason chip; absent for a normal turn. */
  end_reason?: EndReason | null;
}

export type EdgeStatus = "normal" | "consulting" | "needs";

export interface GraphEdge {
  id: string;
  from: string;
  to: string;
  /** `bi=false` → Directed (from→to, single arrowhead), `bi=true` → Consult
   *  (mutual, two arrowheads). Direction is the only edge axis. */
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
