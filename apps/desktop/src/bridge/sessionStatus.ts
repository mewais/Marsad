/**
 * Single source of truth for a session's presentational status.
 *
 * Both the session-detail (transcript) view AND the dashboard derive a session's
 * badge / connection hint / resume-pause affordance / send-ability from THESE
 * functions, fed the SAME canonical `SessionView` (keyed by session id from
 * `state.sessions`). Nothing computes session status from a separate projection
 * (e.g. the workstream-aggregate `WorkstreamCard`) — that is what let the
 * dashboard and detail view drift apart (Unstable badge missing on the board,
 * "Resume" shown on a Done-but-live session). Keep all per-session status logic
 * here so the two views can never diverge again.
 */
import type { SessionView } from "./types";
import { connHintForSession, type ConnHint, type HostLike } from "./connectivity";

/** Paused = not running on its host (authoritative `live` flag, NOT phase). */
export function isPaused(sv: SessionView): boolean {
  return sv.live === false;
}

/** Resume is offered iff the session is Paused. Mirrors SessionDetail exactly. */
export function canResume(sv: SessionView): boolean {
  return isPaused(sv);
}

/** Pause is offered iff the session is running (Resumed). */
export function canPause(sv: SessionView): boolean {
  return !isPaused(sv);
}

/**
 * Per-session connection hint (Reconnecting / Unstable / Disconnected), from the
 * session's own `conn_health` preferred over the coarse host poll. `null` = clean.
 *
 * Liveness-first, exactly like Badge/severity: a PAUSED session has no
 * connectivity state to show — it was stopped on purpose, there is nothing to
 * reconnect to. Returning null here also suppresses the host-status fallback
 * (a paused session on a host that still reads "Connecting" must not show
 * "Reconnecting"), and belt-and-suspenders any stale conn_health that raced a
 * pause on the backend.
 */
export function sessionConnHint(
  sv: SessionView,
  hosts: readonly HostLike[]
): ConnHint | null {
  if (isPaused(sv)) return null;
  return connHintForSession(sv.conn_health, hosts, sv.host_id);
}

/**
 * Whether a prompt/command can be SENT now: the session must be running AND its
 * connection clean (Connected). Paused or Reconnecting/Unstable blocks sending
 * (the composer stays editable; only delivery waits).
 */
export function canSend(sv: SessionView): boolean {
  if (isPaused(sv)) return false;
  return (sv.conn_health ?? "Connected") === "Connected";
}

/** Human reason sending is blocked, or `null` if sending is fine. */
export function sendBlockedReason(sv: SessionView): string | null {
  if (isPaused(sv)) return "Session paused — resume it to send";
  const health = sv.conn_health ?? "Connected";
  if (health === "Reconnecting") return "Reconnecting…";
  if (health === "Unstable") return "Connection unstable…";
  if (health === "Disconnected") return "Disconnected — lost contact with the host…";
  return null;
}

/**
 * Aggregate the resume/pause/conn decision across a set of sessions belonging to
 * one dashboard card (a workstream may hold several). A card can be Paused only
 * when EVERY member is paused; it shows the WORST connection hint among members
 * (Disconnected > Unstable > Reconnecting > clean).
 */
export function aggregateCardStatus(
  sessions: readonly SessionView[],
  hosts: readonly HostLike[]
): { canPause: boolean; canResume: boolean; connHint: ConnHint | null } {
  if (sessions.length === 0) {
    return { canPause: false, canResume: true, connHint: null };
  }
  const anyLive = sessions.some((s) => !isPaused(s));
  const rank: Record<ConnHint, number> = {
    Reconnecting: 1,
    Unstable: 2,
    Disconnected: 3,
  };
  let worst: ConnHint | null = null;
  for (const s of sessions) {
    const h = sessionConnHint(s, hosts);
    if (h && (!worst || rank[h] > rank[worst])) worst = h;
  }
  return { canPause: anyLive, canResume: !anyLive, connHint: worst };
}
