/**
 * The wire, as the UI sees it: every payload type is generated from the Rust crates
 * (`./generated`, read-only here) and re-exported from this module, plus the method and topic
 * names from `marsad-proto` (`methods`, `topics`) and `wasit-runtime::wire`.
 */
export type * from "./generated";
export type { JsonValue } from "./generated/serde_json/JsonValue";
import type { GateRequest, HostId, SessionId, StoredLayer } from "./generated";

/** `hosts.auth_answer` (marsad-proto `hosts::AuthAnswer`; not exported to TS yet). */
export interface AuthAnswer {
  host: HostId;
  id: number;
  answer: string;
}
/** `hosts.auth_cancel` (marsad-proto `hosts::AuthCancel`; not exported to TS yet). */
export interface AuthCancel {
  host: HostId;
  id: number;
}
/** `store.layers.set` params (`pub type LayerSet = StoredLayer`). */
export type LayerSet = StoredLayer;

export type GateOf<K extends GateRequest["gate_type"]> = Extract<GateRequest, { gate_type: K }>;
export type PhaseKind = import("./generated").Phase["state"];

/** UI-only: a session is addressed by host + id. */
export type SessionKey = string;
export const sessionKey = (host: HostId, id: SessionId): SessionKey => `${host}/${id}`;
export function splitKey(key: SessionKey): { host: HostId; id: SessionId } {
  const i = key.indexOf("/");
  return { host: key.slice(0, i), id: key.slice(i + 1) };
}

export const methods = {
  // wasit-runtime (served by every daemon; routed by `host`)
  HOST_INFO: "host.info",
  DRIVERS_INVENTORY: "drivers.inventory",
  DRIVERS_REFRESH: "drivers.refresh",
  SESSIONS_LIST: "sessions.list",
  SESSIONS_CREATE: "sessions.create",
  SESSIONS_CONTINUE: "sessions.continue",
  SESSIONS_STOP: "sessions.stop",
  SESSIONS_DELETE: "sessions.delete",
  SESSION_SNAPSHOT: "session.snapshot",
  SESSION_HISTORY: "session.history",
  SESSION_COMMAND: "session.command",
  // marsad host services
  GIT_STATUS: "git.status",
  GIT_DIFF: "git.diff",
  GIT_COMMIT: "git.commit",
  GIT_DISCARD: "git.discard",
  PTY_START: "pty.start",
  PTY_INPUT: "pty.input",
  PTY_RESIZE: "pty.resize",
  PTY_CLOSE: "pty.close",
  PTY_LIST: "pty.list",
  FS_LIST: "fs.list",
  FS_HOME: "fs.home",
  FS_EXISTS: "fs.exists",
  AGENTCONFIG_BASE: "agentconfig.base",
  DAEMON_INFO: "daemon.info",
  // hub services
  HOSTS_LIST: "hosts.list",
  HOSTS_ADD: "hosts.add",
  HOSTS_UPDATE: "hosts.update",
  HOSTS_REMOVE: "hosts.remove",
  HOSTS_IMPORT_SUGGESTIONS: "hosts.import_suggestions",
  HOSTS_RECONNECT: "hosts.reconnect",
  HOSTS_AUTH_ANSWER: "hosts.auth_answer",
  HOSTS_AUTH_CANCEL: "hosts.auth_cancel",
  AGENTCONFIG_EFFECTIVE: "agentconfig.effective",
  SESSIONS_LAUNCH: "sessions.launch",
  STORE_SETTINGS_GET: "store.settings.get",
  STORE_SETTINGS_SET: "store.settings.set",
  STORE_NOTIFICATIONS_GET: "store.notifications.get",
  STORE_NOTIFICATIONS_SET: "store.notifications.set",
  STORE_LAYERS_LIST: "store.layers.list",
  STORE_LAYERS_GET: "store.layers.get",
  STORE_LAYERS_SET: "store.layers.set",
  STORE_SEEN_LIST: "store.seen.list",
  STORE_SEEN_MARK: "store.seen.mark",
  STORE_SEEN_MARK_ALL: "store.seen.mark_all",
  STORE_RECENT_LIST: "store.recent.list",
  STORE_RECENT_ADD: "store.recent.add",
  STORE_WORKSTREAMS_LIST: "store.workstreams.list",
  STORE_WORKSTREAMS_PUT: "store.workstreams.put",
  STORE_WORKSTREAMS_DELETE: "store.workstreams.delete",
} as const;

export const topics = {
  FLEET: "fleet",
  HOSTS_LINKS: "hosts.links",
  HOST_SESSIONS: "host.sessions",
  DRIVERS_INVENTORY: "drivers.inventory",
  sessionEvents: (id: SessionId) => `session.events:${id}`,
  ptyOutput: (id: string) => `pty.output:${id}`,
} as const;
