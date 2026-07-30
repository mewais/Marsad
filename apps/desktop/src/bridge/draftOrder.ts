/**
 * draftOrder.ts — sorts draft nodes by dependency depth before launch.
 *
 * Mirrors the Rust `dependency_depth` from `crates/marsad-workstream/src/lib.rs`:
 *   - Sources (no inbound edges, depth 0) launch first.
 *   - The lead node (isLead: true) always heads the depth-0 group.
 *   - Within a depth level the AUTHORED ORDER is preserved (stable sort).
 *   - If the graph has a cycle (topological sort cannot complete), fall back
 *     to authored order WITHOUT throwing — the function never throws.
 */

// Re-export DraftNode and DraftEdge so the test can import from this module
// (avoids the test having two separate import sources).
export type { DraftNode, DraftEdge } from "./useVm";

import type { DraftNode, DraftEdge } from "./useVm";

/**
 * Returns a new array of `nodes` sorted by dependency depth (topological
 * order), with the lead at the front of depth-0.  Authored order is preserved
 * within each depth level (stable).  Falls back to authored order on cycle.
 */
export function orderDraftLaunch(
  nodes: DraftNode[],
  edges: DraftEdge[],
): DraftNode[] {
  if (nodes.length === 0) return [];

  // Build adjacency (from → [to]) and in-degree for Kahn's algorithm.
  const nodeIds = new Set(nodes.map((n) => n.tempId));
  const adj = new Map<string, string[]>();
  const inDeg = new Map<string, number>();

  for (const n of nodes) {
    adj.set(n.tempId, []);
    inDeg.set(n.tempId, 0);
  }

  for (const e of edges) {
    // Only count edges whose both endpoints are in our node set.
    if (!nodeIds.has(e.from) || !nodeIds.has(e.to)) continue;
    adj.get(e.from)!.push(e.to);
    inDeg.set(e.to, (inDeg.get(e.to) ?? 0) + 1);
  }

  // Build an ordered index: authored position for each tempId.
  const authoredPos = new Map<string, number>();
  for (let i = 0; i < nodes.length; i++) {
    authoredPos.set(nodes[i].tempId, i);
  }

  // Kahn's topological sort, processing depth level by depth level so that
  // all sources at depth D appear before dependents at depth D+1.
  // Within each level we sort by: lead first, then authored position.
  const sorted: DraftNode[] = [];

  // Initial sources queue (in-degree 0).
  let queue: string[] = [];
  for (const n of nodes) {
    if ((inDeg.get(n.tempId) ?? 0) === 0) queue.push(n.tempId);
  }

  // Sort the initial queue: lead first, then authored order.
  queue = sortIds(queue, nodes, authoredPos);

  const nodeById = new Map<string, DraftNode>();
  for (const n of nodes) nodeById.set(n.tempId, n);

  while (queue.length > 0) {
    // Process the entire current level.
    const nextQueue: string[] = [];
    for (const id of queue) {
      sorted.push(nodeById.get(id)!);
      for (const nb of adj.get(id) ?? []) {
        const newDeg = (inDeg.get(nb) ?? 0) - 1;
        inDeg.set(nb, newDeg);
        if (newDeg === 0) nextQueue.push(nb);
      }
    }
    queue = sortIds(nextQueue, nodes, authoredPos);
  }

  // Cycle fallback: if Kahn didn't drain all nodes, there's a cycle.
  // Return authored order.
  if (sorted.length !== nodes.length) {
    return [...nodes];
  }

  return sorted;
}

/** Sort a list of node ids: lead (isLead=true) first, then by authored position. */
function sortIds(
  ids: string[],
  nodes: DraftNode[],
  authoredPos: Map<string, number>,
): string[] {
  const nodeById = new Map<string, DraftNode>();
  for (const n of nodes) nodeById.set(n.tempId, n);

  return [...ids].sort((a, b) => {
    const na = nodeById.get(a);
    const nb = nodeById.get(b);
    const aLead = na?.isLead ? 0 : 1;
    const bLead = nb?.isLead ? 0 : 1;
    if (aLead !== bLead) return aLead - bLead;
    return (authoredPos.get(a) ?? 0) - (authoredPos.get(b) ?? 0);
  });
}
