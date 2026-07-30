/**
 * connValidate.ts — client-side TS port of the Rust `validate_connection`
 * from `crates/marsad-workstream/src/lib.rs`.
 *
 * Check order mirrors Rust (first failure wins):
 *   1. SelfLoop        — from === to
 *   2. UnknownEndpoint — either endpoint not in the node set
 *   3. CrossWorkstream — either endpoint belongs to a different workstream
 *   4. Duplicate       — identical (from, to) edge already exists
 *   5. Cycle           — adding the edge would create a directed cycle (DFS,
 *                        ignoring `bi` edges — they are not directional deps)
 */

/** The five error kinds mirroring Rust `ConnError`. */
export type ConnErrorKind =
  | "self_loop"
  | "unknown_endpoint"
  | "cross_workstream"
  | "duplicate"
  | "cycle";

/**
 * Human-readable messages pinned to the Rust `ConnError::Display`
 * implementation (lib.rs lines 201-212).
 */
export const CONN_ERROR_MESSAGE: Record<ConnErrorKind, string> = {
  self_loop: "connection endpoints must not be the same node",
  unknown_endpoint: "connection endpoint is not a known node",
  cross_workstream: "connection endpoints span more than one workstream",
  duplicate: "an identical connection already exists",
  cycle: "connection would create a dependency cycle",
};

/** Minimal node shape required by `validateConnection`. The `ws` field is
 *  optional so callers that only have `{id}` nodes still compile (the cross-
 *  workstream check is skipped when `ws` is absent on ALL nodes). */
interface ValidateNode {
  id: string;
  ws?: string;
}

/** Minimal edge shape required by `validateConnection`. */
interface ValidateEdge {
  from: string;
  to: string;
}

/**
 * Validates a proposed new directed edge `from → to` against the current
 * node/edge set (which should be the MERGED draft+real graph for client-side
 * design validation).
 *
 * @param wsId  The workstream being edited.
 * @param from  Source node id.
 * @param to    Target node id.
 * @param nodes All nodes in the merged graph.
 * @param edges All existing directed edges in the merged graph.
 * @returns The first `ConnErrorKind` that applies, or `null` if valid.
 */
export function validateConnection(
  wsId: string,
  from: string,
  to: string,
  nodes: ValidateNode[],
  edges: ValidateEdge[],
): ConnErrorKind | null {
  // 1. SelfLoop — checked first, even before existence.
  if (from === to) return "self_loop";

  // Build a lookup map for O(1) node access.
  const nodeMap = new Map<string, ValidateNode>();
  for (const n of nodes) {
    nodeMap.set(n.id, n);
  }

  // 2. UnknownEndpoint — either endpoint absent from the node set.
  if (!nodeMap.has(from) || !nodeMap.has(to)) return "unknown_endpoint";

  // 3. CrossWorkstream — either endpoint belongs to a different workstream.
  //    We only check when the nodes carry a `ws` field; if they don't, the
  //    single-ws merged view is assumed to be homogeneous.
  const fromNode = nodeMap.get(from)!;
  const toNode = nodeMap.get(to)!;
  if (fromNode.ws !== undefined || toNode.ws !== undefined) {
    if (fromNode.ws !== wsId || toNode.ws !== wsId) {
      return "cross_workstream";
    }
  }

  // 4. Duplicate — identical (from, to) pair already in the edge list.
  for (const e of edges) {
    if (e.from === from && e.to === to) return "duplicate";
  }

  // 5. Cycle — DFS from `to` following existing directed edges; if we can
  //    reach `from`, adding from→to would close a cycle.
  // Build adjacency list (directed only).
  const adj = new Map<string, string[]>();
  for (const n of nodes) {
    adj.set(n.id, []);
  }
  for (const e of edges) {
    const list = adj.get(e.from);
    if (list) list.push(e.to);
  }

  // DFS: can we reach `from` starting from `to`?
  const visited = new Set<string>();
  const stack: string[] = [to];
  while (stack.length > 0) {
    const cur = stack.pop()!;
    if (cur === from) return "cycle";
    if (visited.has(cur)) continue;
    visited.add(cur);
    const neighbours = adj.get(cur) ?? [];
    for (const nb of neighbours) {
      if (!visited.has(nb)) stack.push(nb);
    }
  }

  return null;
}
