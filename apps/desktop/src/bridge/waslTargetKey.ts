/**
 * waslTargetKey.ts — Pure helper for building the `wasl_target_key` object
 * that `launchWaslSession` sends to the agent.
 *
 * Extracted from NodeModal.handleLaunch and GraphScreenConnected.onConfirmDraft
 * so the resume threading logic is testable without React rendering.
 *
 * W2b: thread the chosen native session id into `context.resume` so the Wasl
 * managed runtime resumes that session instead of starting fresh.
 *
 * Contract (mirrors native launchSession resume semantics):
 *   - `resume: false`           → no `resume` key in context (fresh session)
 *   - `resume: true`, `selectedResume === "continue"` → most-recent session id
 *     from the sessions list → `context.resume = sessions[0].id`
 *   - `resume: true`, `selectedResume !== "continue"` → that literal id
 *     → `context.resume = selectedResume`
 *
 * The `waslProtocol` for the driver is taken from the drivers.ts helper
 * (already imported by callers). This helper is PURE: no side effects, no
 * React, no API calls.
 */

/** One A2A peer entry. `agent_id`/`host_id` are the opaque routing ids; `name`/
 *  `host_name` are the USER-facing display names (the peer node's name and its
 *  host's registry label) threaded through so the inner agent's `wasl` MCP
 *  roster shows peers by the names a human gave them. Names are optional for
 *  forward-compat (an older caller may omit them). */
export interface WaslPeer {
  agent_id: string;
  host_id: string;
  name?: string;
  host_name?: string;
}

export interface WaslTargetKeyContext {
  cwd: string;
  model: string;
  session_mode: string;
  peers: WaslPeer[];
  /** The SELF node's host display name (its host's registry label), so the
   *  inner agent can describe its own host by name via `list_peers`. Optional
   *  (forward-compat). */
  host_name?: string;
  resume?: string;
}

export interface WaslTargetKeyShape {
  key: string;
  protocol: string;
  context: WaslTargetKeyContext;
}

export interface DiscoveredSession {
  id: string;
  snippet: string;
  modified_ms: number;
  project?: string | null;
}

/** Minimal session shape: only the id is needed for resume threading. */
type SessionWithId = Pick<DiscoveredSession, "id">;

/**
 * Build the `wasl_target_key` for a Wasl driver launch.
 *
 * @param nodeId   Stable node/session id (used as the key and agent_id).
 * @param protocol The wasl protocol string: "claude" | "opencode" | "codex".
 * @param cwd      The project directory.
 * @param model    The chosen model id.
 * @param sessionMode The chosen session mode id.
 * @param peers    The A2A peer list (each carrying its display name + host name).
 * @param resume   Whether the user enabled resume/continue.
 * @param selectedResume "continue" for most-recent, or a specific session id.
 * @param sessions The session list to pick the most-recent from (when "continue").
 *                 Only `id` is required; accepts full `DiscoveredSession` too.
 * @param selfHostName The SELF node's host display name (registry label), carried
 *                 on `context.host_name` for the inner agent's self identity.
 *                 Optional; omitted from the context when empty/undefined.
 */
export function buildWaslTargetKey(
  nodeId: string,
  protocol: string,
  cwd: string,
  model: string,
  sessionMode: string,
  peers: WaslPeer[],
  resume: boolean,
  selectedResume: string,
  sessions: SessionWithId[],
  selfHostName?: string
): WaslTargetKeyShape {
  const context: WaslTargetKeyContext = {
    cwd,
    model,
    session_mode: sessionMode,
    peers,
  };

  // Carry the self node's host display name when known (forward-compat: omit
  // when empty so an un-upgraded path stays byte-identical).
  if (selfHostName) {
    context.host_name = selfHostName;
  }

  if (resume) {
    if (selectedResume === "continue") {
      // "Continue most recent" → use the most-recent session id (sessions[0]
      // since they come ordered newest-first from capabilities).
      const mostRecent = sessions[0];
      if (mostRecent) {
        context.resume = mostRecent.id;
      }
    } else {
      // A specific session was chosen — use that id directly.
      context.resume = selectedResume;
    }
  }
  // When resume === false: omit `resume` entirely (fresh session).

  return { key: nodeId, protocol, context };
}
