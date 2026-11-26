import type { AuthPrompt, FleetLink, HostEntry, HostId, LinkState } from "../bridge/wire";
import type { HubLink } from "./model";

/**
 * A host's link as the UI uses it. A remote host's is the hub's report. The local host is reached
 * through the app's own link to the local daemon, so its link is that one: `hub` is it (absent
 * where only the fleet is known, which reads as connected).
 */
export type HostLink =
  | { link: "local"; hub?: HubLink }
  /** `stale`: the hub's report is the last one received, not live (the app lost its link to the hub). */
  | { link: "remote"; state: LinkState; stale?: boolean };

/** One host as the fleet reports it (UI state, built from `FleetEvent`s). */
export interface HostState {
  entry: HostEntry;
  link: FleetLink;
  sessionsLive: boolean;
  authPrompt: AuthPrompt | null;
  feedError: string | null;
}

export type LinkTone = "ok" | "warn" | "bad" | "info";
/** A link state reduced to what the UI shows: a key for styling/tests, a tone and words. */
export interface LinkView {
  key: "local" | LinkState["state"] | "signing_in" | "stale";
  tone: LinkTone;
  /** Short words ("Reconnecting"). */
  word: string;
  /** Longer words, may change over time ("Reconnecting in 4s"). Pass `now` to compute. */
  detail: string;
  connected: boolean;
  /** The hub is not reconnecting this host: it waits for the user at sign-in (`auth_needed` with `held`). */
  held?: true;
}

export function nextAtMs(l: Extract<LinkState, { state: "reconnecting" }>): number {
  return l.next_at.secs_since_epoch * 1000 + Math.floor(l.next_at.nanos_since_epoch / 1e6);
}

const PROBLEM: Record<string, string> = {
  auth_needed: "Sign-in did not succeed",
  connection_lost: "Connection lost",
  host_key_unknown: "Host key not trusted yet",
  host_key_changed: "Host key changed",
  unreachable: "Unreachable",
  command_not_found: "Helper not found",
  unknown_distro: "WSL distribution not found",
  wsl_unavailable: "WSL not available",
  permission_denied: "Helper not allowed to run",
  unsupported_platform: "Platform not supported",
  helper_missing: "Helper missing from this install",
  unexpected_output: "Unexpected answer from host",
  helper_failed: "Helper stopped before connecting",
  other: "Cannot connect",
};

/** The app's link to its local daemon, in words (the local host's link). */
export function hubView(hub: HubLink | undefined): LinkView {
  switch (hub?.state) {
    case undefined:
    case "connected":
      return { key: "local", tone: "ok", word: "Connected", detail: "Connected · this machine", connected: true };
    case "connecting":
      return { key: "connecting", tone: "info", word: "Connecting", detail: "Connecting to the local service, starting it if it is not running…", connected: false };
    case "reconnecting":
      return { key: "reconnecting", tone: "warn", word: "Reconnecting", detail: hub.error ? `Reconnecting to the local service: ${hub.error}` : "Reconnecting to the local service…", connected: false };
    case "offline":
      return { key: "failed", tone: "bad", word: "Not connected", detail: hub.error ? `Cannot reach the local service: ${hub.error}` : "Cannot reach the local service", connected: false };
  }
}

export function linkView(link: HostLink, prompt: AuthPrompt | null = null, now = Date.now()): LinkView {
  if (link.link === "local") return hubView(link.hub);
  const v = remoteView(link.state, prompt, now);
  // Last known is not a connection: it cannot be acted on, and it says so instead of "Connected".
  return link.stale ? { key: "stale", tone: "warn", word: `${v.word} (last known)`, detail: `Last known: ${v.word.toLowerCase()}`, connected: false } : v;
}

function remoteView(l: LinkState, prompt: AuthPrompt | null, now: number): LinkView {
  if (prompt) return { key: "signing_in", tone: "warn", word: "Waiting for your answer", detail: "Waiting for your answer", connected: false };
  switch (l.state) {
    case "connected":
      return { key: l.state, tone: "ok", word: "Connected", detail: "Connected", connected: true };
    case "connecting":
      return { key: l.state, tone: "info", word: "Connecting", detail: "Connecting…", connected: false };
    case "provisioning": {
      const w = l.reason.kind === "command_not_found" ? "Installing helper" : "Updating helper";
      return { key: l.state, tone: "info", word: w, detail: `${w}…`, connected: false };
    }
    case "reconnecting": {
      const s = Math.max(1, Math.round((nextAtMs(l) - now) / 1000));
      return { key: l.state, tone: "warn", word: "Reconnecting", detail: nextAtMs(l) > now ? `Reconnecting in ${s}s` : "Reconnecting now", connected: false };
    }
    case "auth_needed":
      // Held: the hub waits for the user (every attempt asks them something), so this is an ask, not an error.
      if (l.held) return { key: l.state, tone: "warn", word: "Needs you to sign in", detail: "Waiting for you to sign in", connected: false, held: true };
      return { key: l.state, tone: "bad", word: PROBLEM[l.problem] ?? "Cannot connect", detail: l.detail || (PROBLEM[l.problem] ?? ""), connected: false };
    case "incompatible":
      return { key: l.state, tone: "bad", word: "Incompatible version", detail: `Helper speaks ${l.theirs.major}.${l.theirs.minor}, Marsad ${l.ours.major}.${l.ours.minor}`, connected: false };
    case "failed":
      return { key: l.state, tone: "bad", word: (l.problem && PROBLEM[l.problem]) || "Failed", detail: l.reason, connected: false };
    case "stopped":
      return { key: l.state, tone: "info", word: "Stopped", detail: "Not connecting", connected: false };
  }
}

/** UI view of a host, stable per HostState object. */
export interface HostView {
  id: HostId;
  name: string;
  local: boolean;
  /** Files of this host are directly reachable from this machine (decided by the hub). */
  localFiles: boolean;
  /** The hub keeps one authenticated ssh connection open to this host (Linux and macOS hubs): it can be signed out of. Decided by the hub, never by the app's own OS. */
  sharedConnection: boolean;
  /** WSL distro name, for path translation. */
  distro: string | null;
  kind: HostEntry["kind"];
  os?: string;
  where: string;
  link: HostLink;
  authPrompt: AuthPrompt | null;
  interactiveAuth: boolean;
  askBeforeConnecting: boolean;
  /** Why the host's session feed is not live (last-known sessions are shown). */
  feedError: string | null;
}

const cache = new WeakMap<HostState, { hub: HubLink; live: boolean; view: HostView }>();
/**
 * UI view of a host, stable while neither the host nor what its link depends on changes: for the
 * local host the app's link to the daemon, for a remote one whether the fleet is live (`live`).
 */
export function hostView(h: HostState, hub: HubLink, live: boolean): HostView {
  const hit = cache.get(h);
  if (hit && hit.live === live && (h.link.link !== "local" || hit.hub === hub)) return hit.view;
  const k = h.entry.kind;
  const v: HostView = {
    id: h.entry.id,
    name: h.entry.name,
    local: k.kind === "local",
    localFiles: h.entry.local_files,
    sharedConnection: h.entry.shared_connection,
    distro: k.kind === "wsl" ? k.distro : null,
    kind: k,
    os: h.entry.platform?.os,
    where: k.kind === "local" ? "Local" : k.kind === "ssh" ? `${k.target}${k.port ? `:${k.port}` : ""}` : `WSL · ${k.distro}`,
    link: h.link.link === "local" ? { link: "local", hub } : { link: "remote", state: h.link.state, ...(live ? {} : { stale: true }) },
    authPrompt: h.authPrompt,
    interactiveAuth: h.entry.interactive_auth,
    askBeforeConnecting: !!h.entry.ask_before_connecting,
    feedError: h.feedError,
  };
  cache.set(h, { hub, live, view: v });
  return v;
}

/** Names of the other hosts that run on the same machine as `id` (same machine key). */
export function sameMachineAs(hosts: Record<string, { entry: HostEntry }>, id: string): string[] {
  const m = hosts[id]?.entry.machine;
  if (!m) return [];
  return Object.values(hosts).filter((h) => h.entry.id !== id && h.entry.machine === m).map((h) => h.entry.name);
}

export function osWord(os?: string): string {
  if (!os) return "";
  return os === "macos" ? "macOS" : os === "windows" ? "Windows" : os === "linux" ? "Linux" : os;
}
