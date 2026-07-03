/**
 * Typed wrappers over @tauri-apps/api invoke() for every bridge command.
 *
 * Browser/test mode: when the Tauri runtime is absent, falls back to the JS
 * mock fixture so Vitest + Playwright work without the Rust side.
 */

import type { AppSnapshot, CapabilitiesView, ModelInfo, PersistedTemplate, SessionSummary, Settings, SideRow, SlashCommandInfo } from "./types";
import { MOCK_SNAPSHOT } from "./mockFixture";
import { mockWorkstreams } from "./mockWorkstreams";
import type { BoardView, ConnKind, GraphView } from "./workstreamTypes";

// ---------------------------------------------------------------------------
// Tauri detection
// ---------------------------------------------------------------------------

function isTauri(): boolean {
  // Tauri v2 always injects `__TAURI_INTERNALS__` into the webview (that's what
  // `invoke()` uses). `__TAURI__` only exists when `app.withGlobalTauri` is
  // enabled, which it is NOT here — so checking only `__TAURI__` made the real
  // app fall back to mock data. Detect either, preferring the internals global.
  return (
    typeof window !== "undefined" &&
    ("__TAURI_INTERNALS__" in window || "__TAURI__" in window)
  );
}

// ---------------------------------------------------------------------------
// Lazy Tauri invoke loader (avoids importing @tauri-apps/api at module level
// so tests that don't have Tauri shims don't blow up)
// ---------------------------------------------------------------------------

type InvokeFn = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

async function getInvoke(): Promise<InvokeFn> {
  if (!isTauri()) {
    throw new Error("Not in Tauri");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke as InvokeFn;
}

// ---------------------------------------------------------------------------
// Default settings (browser/test mode — mirrors Core::default_settings())
// ---------------------------------------------------------------------------

export const DEFAULT_SETTINGS: Settings = {
  notif_prefs: {
    gate_needs_you: { enabled: true,  priority: "urgent", sound: true,  group: false, dnd: false },
    gate_error:     { enabled: true,  priority: "urgent", sound: true,  group: false, dnd: false },
    session_done:   { enabled: true,  priority: "normal", sound: false, group: true,  dnd: true  },
    session_started:{ enabled: false, priority: "low",    sound: false, group: true,  dnd: true  },
    host_unreachable:{ enabled: true, priority: "urgent", sound: true,  group: false, dnd: false },
  },
  play_sound: true,
  group_alerts: true,
  do_not_disturb: false,
  background: "Gradient",
  restore_on_launch: true,
  confirm_before_stop: true,
  default_model: "claude-sonnet-latest",
  default_posture: "ask",
};

// ---------------------------------------------------------------------------
// Mock state (for browser/test mode — mirrors Rust MockState mutations)
// ---------------------------------------------------------------------------

let _mockSnapshot: AppSnapshot = { ...MOCK_SNAPSHOT };

export function _resetMock() {
  _mockSnapshot = { ...MOCK_SNAPSHOT };
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

export async function snapshot(): Promise<AppSnapshot> {
  if (!isTauri()) return _mockSnapshot;
  const invoke = await getInvoke();
  return invoke("snapshot") as Promise<AppSnapshot>;
}

export async function listSessions(
  project: string,
  hostId: string,
): Promise<SessionSummary[]> {
  if (!isTauri()) {
    return [
      { id: "sess-aaaa-1111", snippet: "Add a login endpoint to the API", modified: 1719000000000 },
      { id: "sess-bbbb-2222", snippet: "Refactor the auth middleware", modified: 1718990000000 },
    ];
  }
  const invoke = await getInvoke();
  return invoke("list_sessions", { project, host_id: hostId }) as Promise<SessionSummary[]>;
}

export async function launchSession(args: {
  project: string;
  host_id: string;
  model: string;
  posture: "ask" | "auto" | "full";
  first_prompt?: string;
  resume_session_id?: string;
  continue_recent?: boolean;
  name?: string;
}): Promise<string> {
  if (!isTauri()) return "mock-session-new";
  const invoke = await getInvoke();
  return invoke("launch_session", args) as Promise<string>;
}

export async function resumeSession(id: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("resume_session", { id });
}

export async function attachSession(id: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("attach_session", { id });
}

export async function stopSession(id: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("stop_session", { id });
}

export async function removeSession(id: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("remove_session", { id });
}

export async function sendPrompt(id: string, text: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("send_prompt", { id, text });
}

/**
 * Run a `!<command>` shell command on the session's host (local or remote)
 * and inject the command + output into the session transcript.
 * Does NOT send anything to Claude — this is purely a local/remote shell exec
 * that appends two transcript entries (User + Tool) for display.
 */
export async function runShellCommand(
  sessionId: string,
  command: string
): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("run_shell", { session_id: sessionId, command });
}

export async function interrupt(id: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("interrupt", { id });
}

/** Parse an environment textarea (one `KEY=VALUE` per line) into `[key, value]`
 *  pairs. Blank lines and lines without a `=` (or with an empty key) are ignored. */
export function parseEnv(text: string): Array<[string, string]> {
  const pairs: Array<[string, string]> = [];
  for (const raw of text.split("\n")) {
    let line = raw.trim();
    if (!line) continue;
    // Tolerate a pasted-from-.bashrc `export ` prefix: `export KEY=VALUE` → `KEY=VALUE`.
    if (line.startsWith("export ")) line = line.slice("export ".length).trimStart();
    const eq = line.indexOf("=");
    if (eq <= 0) continue; // no '=' or empty key → ignore
    const key = line.slice(0, eq).trim();
    let value = line.slice(eq + 1).trim();
    // Strip ONE layer of matching surrounding quotes. Users copy values straight
    // from their shell config, e.g. `CLAUDE_CONFIG_DIR="/proj/.../claude"`, where
    // the quotes are shell syntax — NOT part of the value. Keeping them baked the
    // quotes into the exported value (`export K='"/proj/..."'`), so claude looked
    // for a directory literally named with quotes and reported "not logged in".
    if (
      value.length >= 2 &&
      ((value.startsWith('"') && value.endsWith('"')) ||
        (value.startsWith("'") && value.endsWith("'")))
    ) {
      value = value.slice(1, -1);
    }
    if (!key) continue;
    pairs.push([key, value]);
  }
  return pairs;
}

/** Add a remote host (`label`, `user@host`). The optional per-host launch config
 *  (`claudeBin` / `setup` / `env` as `[key, value]` pairs) is applied whenever a
 *  session launches on the host. Rejects with the backend error string (e.g.
 *  duplicate label / malformed target) on failure. */
export async function addHost(
  label: string,
  target: string,
  claudeBin?: string,
  setup?: string,
  env?: Array<[string, string]>,
): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("add_host", {
    label,
    target,
    claude_bin: claudeBin && claudeBin.length > 0 ? claudeBin : null,
    setup: setup && setup.length > 0 ? setup : null,
    env: env ?? [],
  });
}

/** Update an existing host's launch config (`claudeBin` / `setup` / `env`), and
 *  optionally its `target`. Works for any host id INCLUDING `"localhost"` (pass
 *  no `target` to leave it unchanged). */
export async function updateHost(
  id: string,
  claudeBin?: string,
  setup?: string,
  env?: Array<[string, string]>,
  target?: string,
): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("update_host", {
    id,
    claude_bin: claudeBin && claudeBin.length > 0 ? claudeBin : null,
    setup: setup && setup.length > 0 ? setup : null,
    env: env ?? [],
    target: target && target.length > 0 ? target : null,
  });
}

/** Remove a host by id. */
export async function removeHost(id: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("remove_host", { id });
}

/**
 * Open a real OS terminal running an interactive `ssh` login for `target`
 * (an MFA / keyboard-interactive host). Uses the same ControlPath as the
 * automated connections, so once the user completes the login the master
 * socket persists and later headless connections reuse it.
 */
export async function connectHostInteractive(target: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("connect_host_interactive", { target });
}

/**
 * List installed WSL distros as ready-to-add `wsl:<distro>` host targets.
 * Returns [] outside Tauri or when WSL isn't available.
 */
export async function listWslDistros(): Promise<string[]> {
  if (!isTauri()) return [];
  const invoke = await getInvoke();
  return (await invoke("list_wsl_distros")) as string[];
}

export async function answerGate(args: {
  session_id: string;
  decision_id: string;
  allow: boolean;
  deny_reason?: string;
}): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("answer_gate", args);
}

export async function answerQuestion(args: {
  session_id: string;
  request_id: string;
  answers: Record<string, string>;
}): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("answer_question", args);
}

export async function runSlash(id: string, cmd: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("run_slash", { id, cmd });
}

/** Return the current persisted app settings, or defaults if not yet saved. */
export async function getSettings(): Promise<Settings> {
  if (!isTauri()) return { ...DEFAULT_SETTINGS };
  const invoke = await getInvoke();
  return invoke("get_settings") as Promise<Settings>;
}

/** Persist the app settings blob. */
export async function setSettings(settings: Settings): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("set_settings", { settings });
}

export async function getAutostart(): Promise<boolean> {
  if (!isTauri()) return false;
  const invoke = await getInvoke();
  return invoke("get_autostart") as Promise<boolean>;
}

export async function setAutostart(enabled: boolean): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("set_autostart", { enabled });
}

export async function setWorkspace(id: string): Promise<void> {
  if (!isTauri()) {
    _mockSnapshot = { ..._mockSnapshot, active_ws: id };
    return;
  }
  const invoke = await getInvoke();
  await invoke("set_workspace", { id });
}

/** Save the current session list as a named workspace. */
export async function saveWorkspace(
  name: string,
  sessionIds: string[]
): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("save_workspace", { name, session_ids: sessionIds });
}

/** Duplicate an existing workspace by id. */
export async function duplicateWorkspace(id: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("duplicate_workspace", { id });
}

/** Delete a workspace by id. */
export async function deleteWorkspace(id: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("delete_workspace", { id });
}

/** Export a workspace as a `.marsad.json` string (backend-produced). */
export async function exportWorkspace(id: string): Promise<string> {
  if (!isTauri()) return "{}";
  const invoke = await getInvoke();
  return invoke("export_workspace", { id }) as Promise<string>;
}

/** Import a workspace from a `.marsad.json` string. */
export async function importWorkspace(json: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("import_workspace", { json });
}

/**
 * Stage `files` and create a git commit in the session's project directory.
 *
 * Rejects with the backend error string (e.g. "nothing staged", git stderr)
 * on failure. In browser/test mode this is a no-op.
 */
export async function commit(
  sessionId: string,
  message: string,
  files: string[]
): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("commit", { session_id: sessionId, message, files });
}

// ---------------------------------------------------------------------------
// Per-file diff
// ---------------------------------------------------------------------------

/** Raw cell shape as serialized by `marsad_diff::Cell`. */
interface RawCell {
  ln: string;
  sig: string;
  text: string;
  kind: "Context" | "Add" | "Del" | "Empty";
}

/** Raw row shape as serialized by `marsad_diff::SideRow`. */
interface RawSideRow {
  left: RawCell;
  right: RawCell;
}

function rawCellToSideCell(c: RawCell): SideRow["l"] {
  const bg =
    c.kind === "Add"
      ? "rgba(70,192,138,0.12)"
      : c.kind === "Del"
        ? "rgba(224,89,74,0.12)"
        : "transparent";
  const fg =
    c.kind === "Add" ? "#46c08a" : c.kind === "Del" ? "#e0594a" : "var(--txt2)";
  const sigColor =
    c.kind === "Add" ? "#46c08a" : c.kind === "Del" ? "#e0594a" : "var(--txt3)";
  // Empty padding cells render blank.
  const sig = c.kind === "Empty" ? "" : c.sig;
  return { ln: c.ln, sig, sigColor, text: c.text, fg, bg };
}

function rawRowToSideRow(r: RawSideRow): SideRow {
  return { l: rawCellToSideCell(r.left), r: rawCellToSideCell(r.right) };
}

/**
 * Fetch the side-by-side diff for a single changed file in the session's
 * project (local or remote host). Returns styled `SideRow`s ready for
 * `<DiffView>`. Empty array when there's no diff. No-op `[]` outside Tauri.
 */
export async function getFileDiff(
  sessionId: string,
  path: string
): Promise<SideRow[]> {
  if (!isTauri()) return [];
  const invoke = await getInvoke();
  const raw = (await invoke("get_file_diff", {
    session_id: sessionId,
    path,
  })) as RawSideRow[];
  return raw.map(rawRowToSideRow);
}

// ---------------------------------------------------------------------------
// Session config commands
// ---------------------------------------------------------------------------

/**
 * Restart a session with a new model, posture, and disallowed tools.
 *
 * Sends StopSession + LaunchSession (Resume or Continue) with the SAME
 * marsad session id so the UI row is preserved. Tools in `disallowed_tools`
 * are passed as `--disallowedTools` to the restarted claude process.
 */
export async function restartSession(args: {
  id: string;
  model: string;
  posture: "ask" | "auto" | "full";
  disallowed_tools?: string[];
}): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("restart_session", args);
}

/** Persist a named config template (model + posture). */
export async function saveTemplate(args: {
  name: string;
  model: string;
  posture: "ask" | "auto" | "full";
}): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("save_template", args);
}

/** Return all persisted config templates. */
export async function listTemplates(): Promise<PersistedTemplate[]> {
  if (!isTauri()) return [];
  const invoke = await getInvoke();
  return invoke("list_templates") as Promise<PersistedTemplate[]>;
}

// ---------------------------------------------------------------------------
// Event listener for vm-update (Tauri) / no-op in browser
// ---------------------------------------------------------------------------

export type UnlistenFn = () => void;

export async function listenVmUpdate(
  handler: (event: unknown) => void
): Promise<UnlistenFn> {
  if (!isTauri()) {
    // In browser/test mode there are no live events; return a no-op unlisten.
    return () => {};
  }
  const { listen } = await import("@tauri-apps/api/event");
  return listen("vm-update", (event) => handler(event.payload));
}

// ---------------------------------------------------------------------------
// Workstream commands (U-INT-1) — real invoke in Tauri, mock backend otherwise
// ---------------------------------------------------------------------------

export async function createWorkstream(
  id: string,
  name: string,
  goal: string,
  profile?: string | null,
): Promise<void> {
  if (!isTauri()) return mockWorkstreams.createWorkstream(id, name, goal, profile);
  const invoke = await getInvoke();
  await invoke("create_workstream", { id, name, goal, profile: profile ?? null });
}

export async function deleteWorkstream(id: string): Promise<void> {
  if (!isTauri()) return mockWorkstreams.deleteWorkstream(id);
  const invoke = await getInvoke();
  await invoke("delete_workstream", { id });
}

export async function addConnection(
  id: string,
  ws: string,
  from: string,
  to: string,
  kind: ConnKind,
  bi: boolean,
): Promise<void> {
  if (!isTauri()) return mockWorkstreams.addConnection(id, ws, from, to, kind, bi);
  const invoke = await getInvoke();
  await invoke("add_connection", { id, ws, from, to, kind, bi });
}

export async function removeConnection(id: string): Promise<void> {
  if (!isTauri()) return mockWorkstreams.removeConnection(id);
  const invoke = await getInvoke();
  await invoke("remove_connection", { id });
}

export async function createProfile(id: string, name: string): Promise<void> {
  if (!isTauri()) return mockWorkstreams.createProfile(id, name);
  const invoke = await getInvoke();
  await invoke("create_profile", { id, name });
}

export async function deleteProfile(id: string): Promise<void> {
  if (!isTauri()) return mockWorkstreams.deleteProfile(id);
  const invoke = await getInvoke();
  await invoke("delete_profile", { id });
}

export async function assignSession(
  sessionId: string,
  wsId: string,
  lead: boolean,
): Promise<void> {
  if (!isTauri()) return mockWorkstreams.assignSession(sessionId, wsId, lead);
  const invoke = await getInvoke();
  await invoke("assign_session", { session_id: sessionId, ws_id: wsId, lead });
}

export async function setActiveProfile(profile?: string | null): Promise<void> {
  if (!isTauri()) return mockWorkstreams.setActiveProfile(profile);
  const invoke = await getInvoke();
  await invoke("set_active_profile", { profile: profile ?? null });
}

export async function fetchBoard(): Promise<BoardView> {
  if (!isTauri()) return mockWorkstreams.board();
  const invoke = await getInvoke();
  return invoke("board", {}) as Promise<BoardView>;
}

export async function fetchGraph(wsId: string): Promise<GraphView | null> {
  if (!isTauri()) return mockWorkstreams.graph(wsId);
  const invoke = await getInvoke();
  return invoke("graph", { ws_id: wsId }) as Promise<GraphView | null>;
}

// ---------------------------------------------------------------------------
// Slash commands — session-scoped only (docs/specs/dynamic-capabilities.md
// §1.3). No preset catalog: an empty/absent session list is an honest empty
// state, never backfilled.
// ---------------------------------------------------------------------------

/** Map a `SlashCommandInfo` from the session to the `SlashEntry` shape. */
function slashInfoToEntry(info: SlashCommandInfo): { name: string; desc: string } {
  return { name: info.name, desc: info.description };
}

/**
 * Filter slash commands for the given query, using ONLY the session's live
 * `slash_commands` list. `sessionSlash` absent/empty → `[]` (the empty state
 * is first-class UX — see Composer's "waiting for the session to report
 * commands" hint — never a fallback preset catalog).
 */
export function filterSlash(
  query: string,
  sessionSlash?: SlashCommandInfo[],
): { name: string; desc: string }[] {
  const source: { name: string; desc: string }[] = (sessionSlash ?? []).map(slashInfoToEntry);
  if (!query) return source;
  return source.filter((e) => e.name.startsWith(query));
}

// ---------------------------------------------------------------------------
// Capabilities (U2 — dynamic-capabilities.md §3.6) — capabilities()/
// refresh_capabilities() wrappers + shared lookup helpers used by the
// NewSession/Config/Settings model + resume pickers.
// ---------------------------------------------------------------------------

/** Key format for the `capabilities[(driver, host_id)]` useVm slice. */
export function capsKey(driver: string, hostId: string): string {
  return `${driver}::${hostId}`;
}

/**
 * Find the capabilities entry for a given host, regardless of which driver
 * reported it. There is no driver picker in the UI yet (M7d, non-goal), so
 * screens key off host id alone — exactly one entry exists per host today.
 */
export function capsForHostId(
  capabilities: Record<string, CapabilitiesView> | undefined,
  hostId: string,
): CapabilitiesView | undefined {
  if (!capabilities) return undefined;
  return Object.values(capabilities).find((c) => c.host_id === hostId);
}

/** Union of every known host/driver's models, deduped by id (verbatim, first
 *  occurrence wins) — used by the global default-model picker (Settings). */
export function unionModelCatalog(
  capabilities: Record<string, CapabilitiesView> | undefined,
): ModelInfo[] {
  if (!capabilities) return [];
  const seen = new Set<string>();
  const out: ModelInfo[] = [];
  for (const cap of Object.values(capabilities)) {
    for (const m of cap.models.models) {
      if (!seen.has(m.id)) {
        seen.add(m.id);
        out.push(m);
      }
    }
  }
  return out;
}

/** Initial snapshot of every (driver, host) capabilities entry — same
 *  lock-clone-return pattern as `fetchBoard()`. Mock mode: honest empty. */
export async function fetchCapabilities(): Promise<CapabilitiesView[]> {
  if (!isTauri()) return [];
  const invoke = await getInvoke();
  return invoke("capabilities", {}) as Promise<CapabilitiesView[]>;
}

/**
 * Ask the agent to (re)probe capabilities. Fire-and-forget: this resolves as
 * soon as the refresh is QUEUED — results arrive later as `capabilities`
 * vm-update events, never awaited here (U5 responsiveness rule).
 */
export async function refreshCapabilities(args?: {
  host_id?: string;
  driver?: string;
  project?: string;
}): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("refresh_capabilities", {
    host_id: args?.host_id ?? null,
    driver: args?.driver ?? null,
    project: args?.project ?? null,
  });
}
