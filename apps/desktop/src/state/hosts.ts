import type { AuthPrompt, FleetLink, HostEntry, HostId, LinkState } from "../bridge/wire";

/** One host as the fleet reports it (UI state, built from `FleetEvent`s). */
export interface HostState {
  entry: HostEntry;
  link: FleetLink;
  sessionsLive: boolean;
  authPrompt: AuthPrompt | null;
}

export type LinkTone = "ok" | "warn" | "bad" | "info";
/** A link state reduced to what the UI shows: a key for styling/tests, a tone and words. */
export interface LinkView {
  key: "local" | LinkState["state"] | "signing_in";
  tone: LinkTone;
  /** Short words ("Reconnecting"). */
  word: string;
  /** Longer words, may change over time ("Reconnecting in 4s"). Pass `now` to compute. */
  detail: string;
  connected: boolean;
}

export function nextAtMs(l: Extract<LinkState, { state: "reconnecting" }>): number {
  return l.next_at.secs_since_epoch * 1000 + Math.floor(l.next_at.nanos_since_epoch / 1e6);
}

const PROBLEM: Record<string, string> = {
  auth_needed: "Sign-in needed",
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
  other: "Cannot connect",
};

export function linkView(link: FleetLink, prompt: AuthPrompt | null = null, now = Date.now()): LinkView {
  if (link.link === "local") return { key: "local", tone: "ok", word: "Connected", detail: "Connected · this machine", connected: true };
  const l = link.state;
  if (prompt) return { key: "signing_in", tone: "warn", word: "Waiting for sign-in", detail: "Waiting for you to sign in", connected: false };
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
  kind: HostEntry["kind"];
  os?: string;
  where: string;
  link: FleetLink;
  authPrompt: AuthPrompt | null;
  interactiveAuth: boolean;
}

const cache = new WeakMap<HostState, HostView>();
export function hostView(h: HostState): HostView {
  let v = cache.get(h);
  if (!v) {
    const k = h.entry.kind;
    v = {
      id: h.entry.id,
      name: h.entry.name,
      local: k.kind === "local",
      kind: k,
      os: h.entry.platform?.os,
      where: k.kind === "local" ? "Local" : k.kind === "ssh" ? `${k.target}${k.port ? `:${k.port}` : ""}` : `WSL · ${k.distro}`,
      link: h.link,
      authPrompt: h.authPrompt,
      interactiveAuth: h.entry.interactive_auth,
    };
    cache.set(h, v);
  }
  return v;
}

export function osWord(os?: string): string {
  if (!os) return "";
  return os === "macos" ? "macOS" : os === "windows" ? "Windows" : os === "linux" ? "Linux" : os;
}
