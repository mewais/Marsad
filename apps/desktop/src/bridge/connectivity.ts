/**
 * Session connectivity hint — a purely HOST-derived annotation, NOT a lifecycle
 * state. When a host's connection drops, its sessions stay Resumed (the process
 * is still alive on the host — a dropped link is a Marsad-side event, per the
 * two-axis model). This hint surfaces the connectivity truth *on the session*,
 * mirroring the session's host:
 *   - host Connected                    → no hint
 *   - host Connecting / Reconnecting    → "Reconnecting"  (actively re-establishing)
 *   - host Unreachable                  → "Disconnected"  (contact lost)
 *
 * Host status may arrive capitalized (snapshot `HostStatus`) or lowercased
 * (board `HostChip.status`), so matching is case-insensitive.
 */

export type ConnHint = "Reconnecting" | "Disconnected" | "Unstable";

/** Map a host status (any case) to the session-level connectivity hint. */
export function connHintForStatus(status: string | null | undefined): ConnHint | null {
  switch ((status ?? "").toLowerCase()) {
    case "unreachable":
      return "Disconnected";
    case "reconnecting":
    case "connecting":
      return "Reconnecting";
    default:
      // "connected" or unknown → no hint (we can see it fine).
      return null;
  }
}

/**
 * Map the per-session `conn_health` (from the agent's real stream events) to the
 * hint. This is more accurate and prompt than the host poll: a session's stream
 * can drop while its host login node still pings fine, and vice-versa.
 */
export function connHintForConnHealth(health: string | null | undefined): ConnHint | null {
  switch (health) {
    case "Reconnecting":
      return "Reconnecting";
    case "Unstable":
      return "Unstable";
    default:
      // "Connected" or missing → no session-level hint.
      return null;
  }
}

/**
 * Session-level connectivity hint, PREFERRING the per-session `conn_health`
 * (real reader events) and falling back to the coarse host status. This closes
 * the "host reachable but the session's stream dropped" edge case.
 */
export function connHintForSession(
  connHealth: string | null | undefined,
  hosts: readonly HostLike[],
  hostId: string | null | undefined
): ConnHint | null {
  return connHintForConnHealth(connHealth) ?? connHintForHost(hosts, hostId);
}

/** Minimal host shape needed to look up a status by id. */
export interface HostLike {
  id: string;
  status: string;
}

/** Connectivity hint for a session, from its host's live status. */
export function connHintForHost(
  hosts: readonly HostLike[],
  hostId: string | null | undefined
): ConnHint | null {
  if (!hostId) return null;
  const host = hosts.find((h) => h.id === hostId);
  return connHintForStatus(host?.status);
}

/** Tally reconnecting / disconnected across a set of host statuses. */
export function tallyConnHints(statuses: readonly (string | null | undefined)[]): {
  reconnecting: number;
  disconnected: number;
} {
  let reconnecting = 0;
  let disconnected = 0;
  for (const s of statuses) {
    const hint = connHintForStatus(s);
    if (hint === "Reconnecting") reconnecting += 1;
    else if (hint === "Disconnected") disconnected += 1;
  }
  return { reconnecting, disconnected };
}
