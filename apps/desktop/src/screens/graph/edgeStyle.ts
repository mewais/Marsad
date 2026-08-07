/**
 * Pure edge-semantics mapping for the Level 2 Workstream Graph.
 *
 * Design rule (post-#7): a connection has NO "kind" — direction is the only axis
 * (Directed vs Consult), shown by arrowheads (one vs two), not line style. So the
 * stroke is a single SOLID style; only COLOUR (by status) and WIDTH vary. Kept as
 * tiny pure functions (no React/DOM) so the mapping is unit-testable in isolation.
 */
import type { EdgeStatus } from "../../bridge/workstreamTypes";

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

/** Stroke width by status only (kind is gone): a "needs" edge gets a touch
 *  heavier stroke so the attention read is unambiguous even before the gate
 *  diamond registers; everything else is the base width. */
export function edgeStrokeWidth(status: EdgeStatus): number {
  return status === "needs" ? 3 : 2;
}
