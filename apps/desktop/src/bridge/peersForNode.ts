/**
 * peersForNode.ts — TS port of the Rust `marsad-core::peers_for_node`
 * from `crates/marsad-core/src/workstreams.rs`.
 *
 * Pure function: no state mutations, no I/O.
 *
 * `outbound` = nodes that `nodeId` is wired to send to
 *              (edges where `from === nodeId`, collecting `to`).
 * `inbound`  = nodes wired to send to `nodeId`
 *              (edges where `to === nodeId`, collecting `from`).
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
 * @param connections Directed edges, each with `from` and `to` node ids.
 * @returns           `NodePeers` with sorted, de-duplicated outbound and inbound arrays.
 */
export function peersForNode(
  nodeId: string,
  connections: { from: string; to: string }[],
): NodePeers {
  const outboundSet = new Set<string>();
  const inboundSet = new Set<string>();

  for (const c of connections) {
    // Skip self-edges.
    if (c.from === nodeId && c.to === nodeId) continue;

    if (c.from === nodeId) {
      outboundSet.add(c.to);
    }
    if (c.to === nodeId) {
      inboundSet.add(c.from);
    }
  }

  const outbound = [...outboundSet].sort();
  const inbound = [...inboundSet].sort();

  return { outbound, inbound };
}
