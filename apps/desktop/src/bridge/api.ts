import type { Backend, RequestOptions, Unsubscribe } from "./backend";
import type * as W from "./wire";
import { methods as M, topics as T } from "./wire";

/** Method name → [params, result]. The only place method strings meet payload types. */
export interface MethodMap {
  [M.HOST_INFO]: [void, W.HostInfo];
  [M.DRIVERS_INVENTORY]: [W.InventoryRequest, W.InventoryReply];
  [M.DRIVERS_REFRESH]: [W.RefreshRequest, W.Ack];
  [M.SESSIONS_CONTINUE]: [W.SessionRef, W.Ack];
  [M.SESSIONS_STOP]: [W.SessionRef, W.Ack];
  [M.SESSIONS_DELETE]: [W.SessionRef, W.Ack];
  [M.SESSIONS_RENAME]: [W.RenameSession, W.Ack];
  [M.FLEET_GIT_REFRESH]: [W.GitRefresh, W.Ack];
  [M.SESSION_HISTORY]: [W.HistoryRequest, W.HistoryPage];
  [M.SESSION_COMMAND]: [W.CommandRequest, W.CommandReply];
  [M.GIT_STATUS]: [W.GitStatusRequest, W.GitStatus];
  [M.GIT_DIFF]: [W.GitDiffRequest, W.GitDiff];
  [M.GIT_COMMIT]: [W.GitCommitRequest, W.GitCommitReply];
  [M.GIT_DISCARD]: [W.GitDiscardRequest, W.Ack];
  [M.PTY_START]: [W.PtyStartRequest, W.PtyInfo];
  [M.PTY_INPUT]: [W.PtyInputRequest, W.Ack];
  [M.PTY_RESIZE]: [W.PtyResizeRequest, W.Ack];
  [M.PTY_CLOSE]: [W.PtyRef, W.Ack];
  [M.FS_LIST]: [W.FsListRequest, W.FsListing];
  [M.FS_HOME]: [void, W.FsHome];
  [M.FS_EXISTS]: [W.FsPathRequest, W.FsExists];
  [M.AGENTCONFIG_BASE]: [W.BaseRequest, W.BaseConfig];
  [M.DAEMON_INFO]: [void, W.DaemonInfo];
  [M.HOSTS_ADD]: [W.HostDraft, W.HostEntry];
  [M.HOSTS_UPDATE]: [W.HostEntry, W.HostEntry];
  [M.HOSTS_REMOVE]: [W.HostRef, W.Ack];
  [M.HOSTS_IMPORT_SUGGESTIONS]: [void, W.ImportSuggestions];
  [M.HOSTS_WSL_DISTROS]: [void, W.WslDistros];
  [M.HOSTS_RECONNECT]: [W.HostRef, W.Ack];
  [M.HOSTS_AUTH_ANSWER]: [W.AuthAnswer, W.Ack];
  [M.HOSTS_AUTH_CANCEL]: [W.AuthCancel, W.Ack];
  [M.AGENTCONFIG_EFFECTIVE]: [W.EffectiveRequest, W.EffectiveConfig];
  [M.SESSIONS_LAUNCH]: [W.LaunchRequest, W.LaunchReply];
  [M.STORE_SETTINGS_GET]: [void, W.Settings];
  [M.STORE_SETTINGS_SET]: [W.SettingSet, W.Settings];
  [M.STORE_NOTIFICATIONS_GET]: [void, W.NotificationPrefs];
  [M.STORE_NOTIFICATIONS_SET]: [W.NotificationPrefs, W.NotificationPrefs];
  [M.STORE_LAYERS_LIST]: [void, W.LayerList];
  [M.STORE_LAYERS_GET]: [W.LayerScope, W.OverrideLayer];
  /** marsad-proto `store::LayerSet` is `StoredLayer` (an empty layer removes it). */
  [M.STORE_LAYERS_SET]: [W.StoredLayer, W.Ack];
  [M.STORE_SEEN_LIST]: [void, W.SeenMarkers];
  [M.STORE_SEEN_MARK]: [W.SeenMark, W.Ack];
  [M.STORE_SEEN_MARK_ALL]: [void, W.SeenMarkers];
  [M.STORE_RECENT_LIST]: [void, W.RecentProjects];
  [M.STORE_RECENT_ADD]: [W.RecentProject, W.RecentProjects];
  [M.WS_LIST]: [void, W.WorkstreamList];
  [M.WS_GET]: [W.WorkstreamRef, W.Workstream];
  [M.WS_CREATE]: [W.WorkstreamDraft, W.Workstream];
  [M.WS_UPDATE]: [W.WorkstreamUpdate, W.Workstream];
  [M.WS_DELETE]: [W.WorkstreamRef, W.Ack];
  [M.WS_LAUNCH]: [W.WorkstreamRef, W.LaunchWorkstreamReply];
  [M.WS_ADD_NODE]: [W.AddNode, W.Workstream];
  [M.WS_REMOVE_NODE]: [W.NodeRef, W.Workstream];
  [M.WS_CONNECT]: [W.Connect, W.Workstream];
  [M.WS_DISCONNECT]: [W.EdgeRef, W.Workstream];
  [M.WS_SET_POLICY]: [W.Connect, W.Workstream];
  [M.WS_PAUSE]: [W.WorkstreamRef, W.Workstream];
  [M.WS_RESUME]: [W.WorkstreamRef, W.Workstream];
  [M.WS_ARCHIVE]: [W.WorkstreamRef, W.Workstream];
  [M.WS_SAVE_TEMPLATE]: [W.SaveTemplate, W.WorkstreamTemplate];
  [M.WS_FROM_TEMPLATE]: [W.FromTemplate, W.Workstream];
  [M.WS_TEMPLATES_LIST]: [void, W.TemplateList];
  [M.WS_TEMPLATES_DELETE]: [W.TemplateRef, W.Ack];
  [M.WS_HISTORY]: [W.EdgeHistoryRequest, W.EdgeHistoryPage];
}
export type Method = keyof MethodMap;

export function call<K extends Method>(b: Backend, method: K, params: MethodMap[K][0], opts?: RequestOptions): Promise<MethodMap[K][1]> {
  return b.request<MethodMap[K][1]>(method, params, opts);
}

/** Typed wrappers: components never write method or topic strings. */
export function createApi(b: Backend) {
  const on = (host?: W.HostId): RequestOptions | undefined => (host ? { host } : undefined);
  return {
    hostInfo: (host: W.HostId) => call(b, M.HOST_INFO, undefined, { host }),
    inventory: (host: W.HostId, project?: string | null) => call(b, M.DRIVERS_INVENTORY, { project: project ?? null }, { host }),
    refreshDrivers: (host: W.HostId, r: W.RefreshRequest = {}) => call(b, M.DRIVERS_REFRESH, r, { host }),
    command: (host: W.HostId, id: W.SessionId, command: W.Command) => call(b, M.SESSION_COMMAND, { id, command }, { host }),
    history: (host: W.HostId, r: W.HistoryRequest) => call(b, M.SESSION_HISTORY, r, { host }),
    continueSession: (host: W.HostId, id: W.SessionId) => call(b, M.SESSIONS_CONTINUE, { id }, { host }),
    stopSession: (host: W.HostId, id: W.SessionId) => call(b, M.SESSIONS_STOP, { id }, { host }),
    deleteSession: (host: W.HostId, id: W.SessionId) => call(b, M.SESSIONS_DELETE, { id }, { host }),
    renameSession: (host: W.HostId, id: W.SessionId, display_name: string | null) => call(b, M.SESSIONS_RENAME, { id, display_name }, { host }),
    refreshGit: (host: W.HostId, cwd: string) => call(b, M.FLEET_GIT_REFRESH, { host, cwd }),
    launch: (r: W.LaunchRequest) => call(b, M.SESSIONS_LAUNCH, r),
    daemonInfo: (host?: W.HostId) => call(b, M.DAEMON_INFO, undefined, on(host)),
    git: {
      status: (host: W.HostId, target: W.GitTarget) => call(b, M.GIT_STATUS, { target }, { host }),
      diff: (host: W.HostId, r: W.GitDiffRequest) => call(b, M.GIT_DIFF, r, { host }),
      commit: (host: W.HostId, r: W.GitCommitRequest) => call(b, M.GIT_COMMIT, r, { host }),
      discard: (host: W.HostId, r: W.GitDiscardRequest) => call(b, M.GIT_DISCARD, r, { host }),
    },
    pty: {
      start: (host: W.HostId, r: W.PtyStartRequest) => call(b, M.PTY_START, r, { host }),
      input: (host: W.HostId, id: string, data: string) => call(b, M.PTY_INPUT, { id, data }, { host }),
      resize: (host: W.HostId, id: string, cols: number, rows: number) => call(b, M.PTY_RESIZE, { id, cols, rows }, { host }),
      close: (host: W.HostId, id: string) => call(b, M.PTY_CLOSE, { id }, { host }),
      output: (host: W.HostId, id: string, from: number | undefined, cb: (e: W.PtyEvent) => void): Unsubscribe =>
        b.subscribe(T.ptyOutput(id), { host, from }, (e) => cb(e as W.PtyEvent)),
    },
    fs: {
      list: (host: W.HostId, r: W.FsListRequest) => call(b, M.FS_LIST, r, { host }),
      home: (host: W.HostId) => call(b, M.FS_HOME, undefined, { host }),
      exists: (host: W.HostId, path: string) => call(b, M.FS_EXISTS, { path }, { host }),
    },
    config: {
      base: (host: W.HostId, r: W.BaseRequest) => call(b, M.AGENTCONFIG_BASE, r, { host }),
      effective: (r: W.EffectiveRequest) => call(b, M.AGENTCONFIG_EFFECTIVE, r),
    },
    hosts: {
      add: (d: W.HostDraft) => call(b, M.HOSTS_ADD, d),
      update: (e: W.HostEntry) => call(b, M.HOSTS_UPDATE, e),
      remove: (id: W.HostId) => call(b, M.HOSTS_REMOVE, { id }),
      importSuggestions: () => call(b, M.HOSTS_IMPORT_SUGGESTIONS, undefined),
      wslDistros: () => call(b, M.HOSTS_WSL_DISTROS, undefined),
      reconnect: (id: W.HostId) => call(b, M.HOSTS_RECONNECT, { id }),
      authAnswer: (a: W.AuthAnswer) => call(b, M.HOSTS_AUTH_ANSWER, a),
      authCancel: (a: W.AuthCancel) => call(b, M.HOSTS_AUTH_CANCEL, a),
    },
    store: {
      settings: () => call(b, M.STORE_SETTINGS_GET, undefined),
      setSetting: (key: string, value: W.JsonValue | null) => call(b, M.STORE_SETTINGS_SET, { key, value: value as W.JsonValue }),
      notifications: () => call(b, M.STORE_NOTIFICATIONS_GET, undefined),
      setNotifications: (p: W.NotificationPrefs) => call(b, M.STORE_NOTIFICATIONS_SET, p),
      layers: () => call(b, M.STORE_LAYERS_LIST, undefined),
      layer: (scope: W.LayerScope) => call(b, M.STORE_LAYERS_GET, scope),
      setLayer: (l: W.StoredLayer) => call(b, M.STORE_LAYERS_SET, l),
      seen: () => call(b, M.STORE_SEEN_LIST, undefined),
      markSeen: (m: W.SeenMark) => call(b, M.STORE_SEEN_MARK, m),
      markAllSeen: () => call(b, M.STORE_SEEN_MARK_ALL, undefined),
      recent: () => call(b, M.STORE_RECENT_LIST, undefined),
      addRecent: (p: W.RecentProject) => call(b, M.STORE_RECENT_ADD, p),
    },
    ws: {
      list: () => call(b, M.WS_LIST, undefined),
      get: (id: string) => call(b, M.WS_GET, { id }),
      create: (d: W.WorkstreamDraft) => call(b, M.WS_CREATE, d),
      update: (workstream: W.Workstream, expected_revision?: number) => call(b, M.WS_UPDATE, { workstream, expected_revision: expected_revision ?? null }),
      remove: (id: string) => call(b, M.WS_DELETE, { id }),
      launch: (id: string) => call(b, M.WS_LAUNCH, { id }),
      addNode: (r: W.AddNode) => call(b, M.WS_ADD_NODE, r),
      removeNode: (workstream: string, node: string) => call(b, M.WS_REMOVE_NODE, { workstream, node }),
      connect: (r: W.Connect) => call(b, M.WS_CONNECT, r),
      disconnect: (r: W.EdgeRef) => call(b, M.WS_DISCONNECT, r),
      setPolicy: (r: W.Connect) => call(b, M.WS_SET_POLICY, r),
      pause: (id: string) => call(b, M.WS_PAUSE, { id }),
      resume: (id: string) => call(b, M.WS_RESUME, { id }),
      archive: (id: string) => call(b, M.WS_ARCHIVE, { id }),
      saveTemplate: (r: W.SaveTemplate) => call(b, M.WS_SAVE_TEMPLATE, r),
      fromTemplate: (r: W.FromTemplate) => call(b, M.WS_FROM_TEMPLATE, r),
      templates: () => call(b, M.WS_TEMPLATES_LIST, undefined),
      deleteTemplate: (id: string) => call(b, M.WS_TEMPLATES_DELETE, { id }),
      history: (r: W.EdgeHistoryRequest) => call(b, M.WS_HISTORY, r),
    },
    subscribe: {
      fleet: (cb: (e: W.FleetEvent) => void, from?: string): Unsubscribe => b.subscribe(T.FLEET, { from }, (e) => cb(e as W.FleetEvent)),
      workstreams: (cb: (e: W.WorkstreamsEvent) => void, from?: string): Unsubscribe => b.subscribe(T.WORKSTREAMS, { from }, (e) => cb(e as W.WorkstreamsEvent)),
      inventory: (host: W.HostId, cb: (e: W.InventoryReply) => void): Unsubscribe => b.subscribe(T.DRIVERS_INVENTORY, { host }, (e) => cb(e as W.InventoryReply)),
      sessionEvents: (host: W.HostId, id: W.SessionId, from: number, cb: (e: W.SessionEvent) => void): Unsubscribe =>
        b.subscribe(T.sessionEvents(id), { host, from }, (e) => cb(e as W.SessionEvent)),
    },
    on,
  };
}
export type Api = ReturnType<typeof createApi>;

/** The backend's error message, whatever shape the rejection has (RpcError, MarsadError data, Error). */
export function errorMessage(e: unknown): string {
  const r = e as Partial<W.RpcError> | undefined;
  const data = r?.data as Partial<W.MarsadError> & { stderr?: string } | undefined;
  if (data && typeof data === "object" && "marsad_error" in data) {
    const d = data as Record<string, unknown>;
    const m = (d.message ?? d.what ?? d.path ?? d.tool) as string | undefined;
    if (m && r?.message && !r.message.includes(m)) return `${r.message}: ${m}`;
  }
  if (r?.message) return r.message;
  if (e instanceof Error) return e.message;
  return String(e);
}
