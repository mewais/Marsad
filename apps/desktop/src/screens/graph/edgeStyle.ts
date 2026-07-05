/**
 * Pure edge-semantics mapping for the Level 2 Workstream Graph.
 *
 * Design rule (docs/design/DESIGN_HANDOFF.md §"LEVEL 2" → Edges): **colour
 * encodes status, dash encodes type — never mix the two.** Kept as tiny pure
 * functions (no React/DOM) so the mapping itself is unit-testable in
 * isolation from rendering.
 */
import type { ConnKind, EdgeStatus } from "../../bridge/workstreamTypes";

/** Edge colour is driven PURELY by status: blue=normal, grey=consulting
 *  (two-way), amber=needs (an unresolved gate sits on the wire). */
export function edgeColor(status: EdgeStatus): string {
  switch (status) {
    case "consulting":
      return "var(--txt2)";
    case "needs":
      return "var(--needs)";
    default:
      return "var(--accent)";
  }
}

export interface EdgeDashStyle {
  /** `undefined` = solid stroke ("both"). */
  dasharray: string | undefined;
  /** Marching-ants "flow" animation — data/message edges only. */
  animated: boolean;
  linecap: "round" | "butt";
}

/**
 * Dash pattern is driven PURELY by connection kind (never status):
 * - `data`    → dashed `8 7`, animated (marching ants)
 * - `command` → dotted `2 5`, round caps, static
 * - `both`    → solid, static
 */
export function edgeDashStyle(kind: ConnKind): EdgeDashStyle {
  if (kind === "command") {
    return { dasharray: "2 5", animated: false, linecap: "round" };
  }
  if (kind === "both") {
    return { dasharray: undefined, animated: false, linecap: "butt" };
  }
  // "data" (and any other/unknown kind defaults to the data/message style —
  // the most common connection, never renders as blank).
  return { dasharray: "8 7", animated: true, linecap: "butt" };
}

/** Stroke width per kind, mirroring the design reference's inline styles
 *  (both=2.6, command=2, data=2/2.4-when-attention). Status "needs" gets a
 *  touch heavier stroke so the attention read is unambiguous even before the
 *  gate diamond registers. */
export function edgeStrokeWidth(kind: ConnKind, status: EdgeStatus): number {
  if (kind === "both") return 2.6;
  if (status === "needs") return 3;
  return 2;
}
