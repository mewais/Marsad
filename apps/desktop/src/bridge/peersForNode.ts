/**
 * peersForNode.ts — TS port of the Rust `marsad-core::peers_for_node`
 * from `crates/marsad-core/src/workstreams.rs`.
 *
 * Pure function: no state mutations, no I/O.
 *
 * Direction is the only edge axis (post-#7 — no "kind"):
 *   • Directed A→B (`bi=false`): A drives B — A initiates, B only answers.
 *   • Consult  A↔B (`bi=true`) : mutual — both can initiate.
 *
 * `outbound` = nodes `nodeId` can SEND to (initiate toward):
 *              `to` of every edge where `from === nodeId`, PLUS
 *              `from` of every CONSULT edge where `to === nodeId` (the reverse
 *              leg of a two-way wire).
 * `inbound`  = nodes that can send to `nodeId`:
 *              `from` of every edge where `to === nodeId`, PLUS
 *              `to` of every CONSULT edge where `from === nodeId`.
 *
 * Both arrays are de-duplicated and sorted ascending for determinism.
 * A self-edge (`from === to === nodeId`) is ignored — a node is not its own peer.
 */

export interface NodePeers {
  outbound: string[];
  inbound: string[];
}

/**
 * Return the peer topology for `nodeId` derived from the given connections.
 *
 * @param nodeId      The node whose peers to compute.
 * @param connections Directed edges; each has `from`, `to`, and `bi` (consult).
 * @returns           `NodePeers` with sorted, de-duplicated outbound and inbound arrays.
 */
export function peersForNode(
  nodeId: string,
  connections: { from: string; to: string; bi?: boolean }[],
): NodePeers {
  const outboundSet = new Set<string>();
  const inboundSet = new Set<string>();

  for (const c of connections) {
    // Skip self-edges.
    if (c.from === nodeId && c.to === nodeId) continue;

    if (c.from === nodeId) {
      outboundSet.add(c.to);
      if (c.bi) inboundSet.add(c.to); // consult: the target can also initiate back
    }
    if (c.to === nodeId) {
      inboundSet.add(c.from);
      if (c.bi) outboundSet.add(c.from); // consult: we can initiate toward the source
    }
  }

  const outbound = [...outboundSet].sort();
  const inbound = [...inboundSet].sort();

  return { outbound, inbound };
}
