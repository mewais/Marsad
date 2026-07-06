/**
 * Typed wrappers over @tauri-apps/api invoke() for every bridge command.
 *
 * Browser/test mode: when the Tauri runtime is absent, falls back to the JS
 * mock fixture so Vitest + Playwright work without the Rust side.
 */

import type { AppSnapshot, CapabilitiesView, CostRow, ModelInfo, PersistedTemplate, PtyEvent, SessionSummary, Settings, SideRow, SlashCommandInfo, WorkstreamCostBreakdown } from "./types";
import { MOCK_SNAPSHOT } from "./mockFixture";
import { mockWorkstreams } from "./mockWorkstreams";
import type { BoardView, ConnKind, Driver, GraphView } from "./workstreamTypes";

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
  // #2 new fields: default_driver + default_models (absent ⇒ "claude"/[])
  default_driver: "claude",
  default_models: [],
};

// ---------------------------------------------------------------------------
// Mock state (for browser/test mode — mirrors Rust MockState mutations)
// ---------------------------------------------------------------------------

let _mockSnapshot: AppSnapshot = { ...MOCK_SNAPSHOT };

export function _resetMock() {
  _mockSnapshot = { ...MOCK_SNAPSHOT };
  resolvedGateLog.length = 0;
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
  // X4 launch-time Wasl attach — omitted ⇒ a plain, Wasl-free session.
  wasl?: boolean;
  // #3 agent driver override — omitted ⇒ "claude".
  driver?: string;
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

// ---------------------------------------------------------------------------
// WAVE 3b — pty escape-hatch terminal (Terminal tab). No-op in browser/mock
// mode (there is no backend to talk to — `TerminalTab` shows an honestly
// idle terminal instead of fake output). Deliberately no `ptyStop`: the pty
// is torn down by the agent on session stop, not on leaving the Terminal
// tab — see `bridge.rs`'s WAVE 3b section header for the full rationale.
// ---------------------------------------------------------------------------

/** Start (or, if already live, no-op re-attach to) a session's interactive pty. */
export async function ptyStart(
  sessionId: string,
  cols: number,
  rows: number
): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("pty_start", { session_id: sessionId, cols, rows });
}

/** Forward raw keystroke bytes (from xterm.js's `onData`) to the pty stdin. */
export async function ptyInput(sessionId: string, data: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("pty_input", { session_id: sessionId, data });
}

/** Resize the pty viewport (e.g. after `xterm-addon-fit` recomputes cols/rows). */
export async function ptyResize(
  sessionId: string,
  cols: number,
  rows: number
): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("pty_resize", { session_id: sessionId, cols, rows });
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
 *  session launches on the host. `agentCommands` is a list of `[driver, command]`
 *  pairs overriding the binary used for each driver on this host.
 *  Rejects with the backend error string (e.g. duplicate label / malformed target)
 *  on failure. */
export async function addHost(
  label: string,
  target: string,
  claudeBin?: string,
  setup?: string,
  env?: Array<[string, string]>,
  agentCommands?: Array<[string, string]>,
): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("add_host", {
    label,
    target,
    claude_bin: claudeBin && claudeBin.length > 0 ? claudeBin : null,
    setup: setup && setup.length > 0 ? setup : null,
    env: env ?? [],
    agent_commands: agentCommands ?? [],
  });
}

/** Update an existing host's launch config (`claudeBin` / `setup` / `env` /
 *  `agentCommands`), and optionally its `target`. Works for any host id INCLUDING
 *  `"localhost"` (pass no `target` to leave it unchanged). `agentCommands` is a
 *  list of `[driver, command]` pairs overriding the binary for each driver. */
export async function updateHost(
  id: string,
  claudeBin?: string,
  setup?: string,
  env?: Array<[string, string]>,
  target?: string,
  agentCommands?: Array<[string, string]>,
): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("update_host", {
    id,
    claude_bin: claudeBin && claudeBin.length > 0 ? claudeBin : null,
    setup: setup && setup.length > 0 ? setup : null,
    env: env ?? [],
    target: target && target.length > 0 ? target : null,
    agent_commands: agentCommands ?? [],
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

// ---------------------------------------------------------------------------
// Resolved-gate history (Workstreams redesign SLICE D) — a tiny in-memory,
// module-scoped log of HITL decisions, appended whenever answerGate /
// answerQuestion is called (regardless of caller — today that's exclusively
// GateOverlay's Approve/Reject/Submit actions). This drives the Activity
// tab's "needs-you → resolved" collapse: once a gate/question resolves, the
// pending card disappears (the backend clears `session.gate`), but the
// collapsed summary row needs to know WHAT was decided — and the backend
// does not retain a decision history yet. Purely additive/local (lost on
// reload), matching the "this session's transcript" scope of a redesign
// polish slice rather than a new persisted backend feature.
// ---------------------------------------------------------------------------

export interface ResolvedGateEntry {
  session_id: string;
  decision_id: string;
  resolved_at: number;
  /** Set by answerGate — covers permission/plan/edit gates alike. */
  allow?: boolean;
  deny_reason?: string;
  /** Set by answerQuestion — question text -> picked/typed answer. */
  answers?: Record<string, string>;
}

const resolvedGateLog: ResolvedGateEntry[] = [];

/** All resolved-gate entries for a session, oldest → newest. */
export function getResolvedGates(sessionId: string): ResolvedGateEntry[] {
  return resolvedGateLog.filter((e) => e.session_id === sessionId);
}

/** Test-only: clear the resolved-gate log between test cases. */
export function _resetResolvedGates(): void {
  resolvedGateLog.length = 0;
}

export async function answerGate(args: {
  session_id: string;
  decision_id: string;
  allow: boolean;
  deny_reason?: string;
}): Promise<void> {
  resolvedGateLog.push({
    session_id: args.session_id,
    decision_id: args.decision_id,
    resolved_at: Date.now(),
    allow: args.allow,
    deny_reason: args.deny_reason,
  });
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("answer_gate", args);
}

/**
 * X5 — relay the HUMAN's decision on a Wasl handoff gate to the agent daemon,
 * which POSTs it to the Wasl operator surface. Marsad never auto-answers;
 * this is only ever called from an explicit Approve/Deny click.
 * `outcome` is "approved" | "denied" | "dismissed".
 */
export async function answerWaslGate(args: {
  host_id: string;
  gate_id: string;
  outcome: string;
  reason?: string;
}): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("answer_wasl_gate", args);
}

export async function answerQuestion(args: {
  session_id: string;
  request_id: string;
  answers: Record<string, string>;
}): Promise<void> {
  resolvedGateLog.push({
    session_id: args.session_id,
    decision_id: args.request_id,
    resolved_at: Date.now(),
    answers: args.answers,
  });
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

/**
 * WAVE 3b (pty escape-hatch terminal) — listen on the SEPARATE `"pty"` Tauri
 * event channel (not `"vm-update"`): raw terminal bytes have no business
 * meaning for the board/session/workstream VM `useVm` reduces, so the agent
 * bridge emits them on their own channel (see `events::emit_pty_update`'s
 * Rust doc comment). No-op (never-firing) unlisten in browser/mock mode —
 * `TerminalTab` renders an honestly-idle terminal there instead of fake
 * output.
 */
export async function listenPty(
  handler: (event: PtyEvent) => void
): Promise<UnlistenFn> {
  if (!isTauri()) {
    return () => {};
  }
  const { listen } = await import("@tauri-apps/api/event");
  return listen<PtyEvent>("pty", (event) => handler(event.payload));
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

/**
 * Pause all sessions in a workstream (stop each running process; they remain
 * resumable). Returns immediately; VmUpdates arrive via `listenVmUpdate`.
 */
export async function pauseWorkstream(id: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("pause_workstream", { id });
}

/**
 * Resume all sessions in a workstream (send ResumeSession IPC for each;
 * phases flip to Starting). Returns immediately; VmUpdates arrive via `listenVmUpdate`.
 */
export async function resumeWorkstream(id: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("resume_workstream", { id });
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
// Cost breakdown (SLICE G / Wave 3a) — Usage tab's "Cost breakdown" (By host /
// By node). Real mode forwards to `Core::cost_breakdown` via the `cost_breakdown`
// Tauri command; browser/test mode builds a realistic mock from the same
// mock-workstream graph `fetchGraph` reads, joined against the mock session
// fixture's cost fields — mirroring exactly what `workstream_cost_breakdown`
// does server-side (join nodes_of(ws_id) with SessionView by id).
// ---------------------------------------------------------------------------

/** `cost / total * 100`, or `0` when `total === 0` (avoids a NaN divide) —
 *  mirrors `marsad_viewmodel::cost_pct`. */
function costPct(cost: number, total: number): number {
  return total === 0 ? 0 : (cost / total) * 100;
}

/** Sort rows by `cost_usd` descending, ties broken by `key` ascending —
 *  mirrors `marsad_viewmodel::sort_cost_rows_desc`. */
function sortCostRowsDesc(rows: CostRow[]): CostRow[] {
  return [...rows].sort((a, b) => b.cost_usd - a.cost_usd || a.key.localeCompare(b.key));
}

/** Distinct drivers in first-appearance order — mirrors `marsad_workstream::driver_dots`. */
function dedupDriversFirstAppearance(drivers: Driver[]): Driver[] {
  const seen = new Set<Driver>();
  const out: Driver[] = [];
  for (const d of drivers) {
    if (!seen.has(d)) {
      seen.add(d);
      out.push(d);
    }
  }
  return out;
}

/**
 * Build a realistic mock `WorkstreamCostBreakdown` for browser/test-mode
 * preview: reads the same mock graph `fetchGraph`/`mockWorkstreams.graph`
 * exposes (nodes carry `id`/`name`/`driver`/`host`), joins each node's cost
 * from the mock session fixture (`_mockSnapshot.sessions`, matched by id —
 * exactly how `Core::cost_breakdown` joins nodes to `SessionView`s), and
 * marks the single lowest (nonzero) cost contributor "estimated" so the
 * preview shows the U4 "~ estimated" affordance without lying about which
 * real session it came from (the mock fixture's sessions are all
 * `cost_estimated: false` — real live-reported cost — so this flag exists
 * only in the derived `CostRow`, never mutates the underlying session data).
 */
function mockCostBreakdown(wsId: string): WorkstreamCostBreakdown | null {
  const graph = mockWorkstreams.graph(wsId);
  if (!graph) return null;

  const contributions = graph.nodes.map((n) => {
    const sv = _mockSnapshot.sessions.find((s) => s.id === n.id);
    return { node: n, cost: sv?.cost_usd ?? 0 };
  });

  // Pick the lowest nonzero contributor as the mock's one "estimated" example
  // (deterministic: ties broken by node id) — purely a preview affordance,
  // not a claim about the real session's provenance.
  const nonZero = contributions.filter((c) => c.cost > 0);
  const estimatedNodeId = nonZero.length
    ? nonZero.reduce((min, c) =>
        c.cost < min.cost || (c.cost === min.cost && c.node.id < min.node.id) ? c : min
      ).node.id
    : null;

  const totalCost = contributions.reduce((a, c) => a + c.cost, 0);

  const byNode: CostRow[] = contributions.map(({ node, cost }) => ({
    key: node.id,
    label: node.name,
    driver_dots: [node.driver],
    cost_usd: cost,
    pct: costPct(cost, totalCost),
    estimated: node.id === estimatedNodeId,
  }));

  const hostOrder: string[] = [];
  const hostCost = new Map<string, number>();
  const hostDrivers = new Map<string, Driver[]>();
  const hostAllEstimated = new Map<string, boolean>();
  const hostHasAny = new Map<string, boolean>();
  for (const { node, cost } of contributions) {
    if (!hostCost.has(node.host)) {
      hostOrder.push(node.host);
      hostCost.set(node.host, 0);
      hostDrivers.set(node.host, []);
      hostAllEstimated.set(node.host, true);
      hostHasAny.set(node.host, false);
    }
    hostCost.set(node.host, hostCost.get(node.host)! + cost);
    hostDrivers.get(node.host)!.push(node.driver);
    hostHasAny.set(node.host, true);
    if (node.id !== estimatedNodeId) hostAllEstimated.set(node.host, false);
  }

  const byHost: CostRow[] = hostOrder.map((host) => ({
    key: host,
    label: host,
    driver_dots: dedupDriversFirstAppearance(hostDrivers.get(host) ?? []),
    cost_usd: hostCost.get(host) ?? 0,
    pct: costPct(hostCost.get(host) ?? 0, totalCost),
    estimated: !!hostHasAny.get(host) && !!hostAllEstimated.get(host),
  }));

  return {
    total_cost_usd: totalCost,
    total_estimated: contributions.length > 0 && estimatedNodeId !== null && nonZero.length === 1,
    by_host: sortCostRowsDesc(byHost),
    by_node: sortCostRowsDesc(byNode),
  };
}

export async function fetchCostBreakdown(wsId: string): Promise<WorkstreamCostBreakdown | null> {
  if (!isTauri()) return mockCostBreakdown(wsId);
  const invoke = await getInvoke();
  return invoke("cost_breakdown", { ws_id: wsId }) as Promise<WorkstreamCostBreakdown | null>;
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
// Browser/test-mode capability data — a stand-in for what the real Rust
// capabilities probe returns, so the driver/model pickers render a realistic
// working state in preview (claude launchable; codex/gemini discovered-but-not-
// launchable). These are MOCK BACKEND DATA (the pickers read them dynamically),
// NOT a hardcoded UI preset — model ids use the discovered "-latest" alias form,
// never the versioned literals the kill-list forbids.
const MOCK_HOSTS = ["localhost", "hetzner-cx51", "gpu-worker", "build-host", "wsl-dev"];
function mockCaps(): CapabilitiesView[] {
  const claudeModels: ModelInfo[] = [
    { id: "claude-opus-latest" },
    { id: "claude-sonnet-latest" },
    { id: "claude-haiku-latest" },
  ];
  const out: CapabilitiesView[] = [];
  for (const host of MOCK_HOSTS) {
    out.push({
      driver: "claude", host_id: host, cli_available: true, launchable: true,
      models: { models: claudeModels, source: "Listed", free_text: true },
      session_scope: null, sessions: [], fetched_at_ms: Date.now(), errors: [],
    });
    // codex has a launch driver (launchable) but its CLI is not detected in
    // this mock host; gemini has no launch driver at all (launchable: false).
    for (const driver of ["codex", "gemini"]) {
      out.push({
        driver, host_id: host, cli_available: false, launchable: driver === "codex",
        models: { models: [], source: "Observed", free_text: true },
        session_scope: null, sessions: [], fetched_at_ms: 0, errors: [],
      });
    }
  }
  return out;
}

export async function fetchCapabilities(): Promise<CapabilitiesView[]> {
  if (!isTauri()) return mockCaps();
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
