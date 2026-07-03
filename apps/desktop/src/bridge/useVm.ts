/**
 * useVm — app-wide state store seeded from snapshot() and updated
 * by "vm-update" Tauri events. This is THE single source of truth
 * for all screens. Selectors are derived synchronously (no extra state).
 */

import React, {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useReducer,
} from "react";
import type {
  AppSnapshot,
  CapabilitiesView,
  Counts,
  Filter,
  GateVM,
  Host,
  OverviewGroups,
  Phase,
  SessionCard,
  SessionDetailVM,
  SessionRow,
  SessionView,
  Tab,
  VmUpdateEvent,
  Workspace,
} from "./types";
import {
  listenVmUpdate,
  snapshot as fetchSnapshot,
  fetchBoard,
  fetchGraph,
  fetchCapabilities,
  capsKey,
  createWorkstream as apiCreateWorkstream,
  deleteWorkstream as apiDeleteWorkstream,
  addConnection as apiAddConnection,
  removeConnection as apiRemoveConnection,
  createProfile as apiCreateProfile,
  deleteProfile as apiDeleteProfile,
  assignSession as apiAssignSession,
  setActiveProfile as apiSetActiveProfile,
} from "./api";
import type { BoardView, ConnKind, GraphView } from "./workstreamTypes";

// ---------------------------------------------------------------------------
// Screen / nav state (UI only — not in AppSnapshot)
// ---------------------------------------------------------------------------

export type Screen =
  | "overview"
  | "session"
  | "gate"
  | "new-session"
  | "settings"
  | "workspaces"
  | "board"
  | "graph";

// ---------------------------------------------------------------------------
// Store state
// ---------------------------------------------------------------------------

export interface VmState {
  snapshot: AppSnapshot | null;
  // UI-only nav
  screen: Screen;
  selectedId: string | null;
  tab: Tab;
  filter: Filter;
  search: string;
  wsMenuOpen: boolean;
  // Workstream board/graph nav (U-INT-2)
  board: BoardView | null;
  wsId: string | null;
  graph: GraphView | null;
  nodeSel: string | null;
  activeProfile: string | null;
  // Capabilities (U2), keyed by capsKey(driver, host_id) — dynamic-capabilities.md §3.6.
  capabilities: Record<string, CapabilitiesView>;
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

type Action =
  | { type: "SNAPSHOT_LOADED"; payload: AppSnapshot }
  | { type: "VM_UPDATE"; payload: VmUpdateEvent }
  | { type: "CAPABILITIES_LOADED"; payload: CapabilitiesView[] }
  | { type: "SET_SCREEN"; screen: Screen }
  | { type: "SELECT_SESSION"; id: string | null }
  | { type: "SET_TAB"; tab: Tab }
  | { type: "SET_FILTER"; filter: Filter }
  | { type: "SET_SEARCH"; search: string }
  | { type: "SET_WORKSPACE"; id: string }
  | { type: "TOGGLE_WS_MENU" }
  | { type: "CLOSE_WS_MENU" }
  | { type: "SET_BOARD"; board: BoardView }
  | { type: "SET_GRAPH"; wsId: string; graph: GraphView | null }
  | { type: "OPEN_BOARD" }
  | { type: "OPEN_GRAPH"; wsId: string }
  | { type: "SELECT_NODE"; id: string | null }
  | { type: "SET_ACTIVE_PROFILE"; profile: string | null };

// ---------------------------------------------------------------------------
// Reducer
// ---------------------------------------------------------------------------

function reducer(state: VmState, action: Action): VmState {
  switch (action.type) {
    case "SNAPSHOT_LOADED":
      return {
        ...state,
        snapshot: action.payload,
        filter: action.payload.filter,
        search: action.payload.search,
        tab: action.payload.tab,
        selectedId: action.payload.selected_id,
      };

    case "VM_UPDATE": {
      if (!state.snapshot) return state;
      const ev = action.payload;
      if (ev.kind === "session") {
        // Upsert: update an existing session, or append a newly-launched one
        // (its id won't be in the snapshot that was fetched before launch).
        const exists = state.snapshot.sessions.some((s) => s.id === ev.session.id);
        const sessions = exists
          ? state.snapshot.sessions.map((s) =>
              s.id === ev.session.id ? ev.session : s
            )
          : [...state.snapshot.sessions, ev.session];
        return { ...state, snapshot: { ...state.snapshot, sessions } };
      }
      if (ev.kind === "session_removed") {
        // Drop the removed session's row. If it was the open one, fall back to
        // the overview so we don't strand the user on a deleted session.
        const sessions = state.snapshot.sessions.filter((s) => s.id !== ev.id);
        const wasSelected = state.selectedId === ev.id;
        return {
          ...state,
          snapshot: { ...state.snapshot, sessions },
          selectedId: wasSelected ? null : state.selectedId,
          screen: wasSelected && state.screen === "session" ? "overview" : state.screen,
        };
      }
      if (ev.kind === "overview") {
        // Overview groups are derived; no stored state change needed.
        return state;
      }
      if (ev.kind === "host_health") {
        const hosts = state.snapshot.hosts.map((h) =>
          h.id === ev.host_id
            ? { ...h, status: ev.status as Host["status"] }
            : h
        );
        return { ...state, snapshot: { ...state.snapshot, hosts } };
      }
      if (ev.kind === "hosts") {
        return { ...state, snapshot: { ...state.snapshot, hosts: ev.hosts } };
      }
      if (ev.kind === "workspaces") {
        return {
          ...state,
          snapshot: { ...state.snapshot, workspaces: ev.workspaces },
        };
      }
      if ((ev as { kind: string }).kind === "board") {
        return { ...state, board: (ev as unknown as { board: BoardView }).board };
      }
      if ((ev as { kind: string }).kind === "workstreams") {
        // No direct state field beyond board; refresh is handled by the
        // command helpers that trigger this event. Nothing to do here.
        return state;
      }
      if (ev.kind === "capabilities") {
        // Always a COMPLETE snapshot for this (driver, host) — replace, not
        // merge, on the receiving side (dynamic-capabilities.md §2).
        const key = capsKey(ev.capabilities.driver, ev.capabilities.host_id);
        return {
          ...state,
          capabilities: { ...state.capabilities, [key]: ev.capabilities },
        };
      }
      return state;
    }

    case "CAPABILITIES_LOADED": {
      const capabilities: Record<string, CapabilitiesView> = {};
      for (const c of action.payload) {
        capabilities[capsKey(c.driver, c.host_id)] = c;
      }
      return { ...state, capabilities };
    }

    case "SET_SCREEN":
      return { ...state, screen: action.screen };

    case "SELECT_SESSION":
      return {
        ...state,
        selectedId: action.id,
        screen: action.id ? "session" : state.screen,
      };

    case "SET_TAB":
      return { ...state, tab: action.tab };

    case "SET_FILTER":
      return { ...state, filter: action.filter };

    case "SET_SEARCH":
      return { ...state, search: action.search };

    case "SET_WORKSPACE":
      if (!state.snapshot) return state;
      return {
        ...state,
        snapshot: { ...state.snapshot, active_ws: action.id },
        wsMenuOpen: false,
      };

    case "TOGGLE_WS_MENU":
      return { ...state, wsMenuOpen: !state.wsMenuOpen };

    case "CLOSE_WS_MENU":
      return { ...state, wsMenuOpen: false };

    case "SET_BOARD":
      return { ...state, board: action.board };

    case "SET_GRAPH":
      return { ...state, wsId: action.wsId, graph: action.graph };

    case "OPEN_BOARD":
      return {
        ...state,
        screen: "board",
        wsId: null,
        graph: null,
        nodeSel: null,
      };

    case "OPEN_GRAPH":
      return {
        ...state,
        screen: "graph",
        wsId: action.wsId,
        nodeSel: null,
      };

    case "SELECT_NODE":
      return { ...state, nodeSel: action.id };

    case "SET_ACTIVE_PROFILE":
      return { ...state, activeProfile: action.profile };

    default:
      return state;
  }
}

const initialState: VmState = {
  snapshot: null,
  screen: "overview",
  selectedId: null,
  tab: "Activity",
  filter: "All",
  search: "",
  wsMenuOpen: false,
  board: null,
  wsId: null,
  graph: null,
  nodeSel: null,
  activeProfile: null,
  capabilities: {},
};

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

export interface VmContextValue {
  state: VmState;
  dispatch: React.Dispatch<Action>;
  // Selectors
  counts: Counts;
  overviewGroups: OverviewGroups;
  sidebarList: SessionRow[];
  selectedSession: SessionView | null;
  selectedDetail: SessionDetailVM | null;
  selectedGate: GateVM | null;
  workspaces: Workspace[];
  hosts: Host[];
  activeWs: Workspace | null;
  // Workstream board/graph command helpers (U-INT-2)
  openBoard: () => void;
  openWorkstream: (id: string) => Promise<void>;
  selectNode: (id: string | null) => void;
  openNode: (sessionId: string) => void;
  createWorkstream: (
    id: string,
    name: string,
    goal: string,
    profile?: string | null
  ) => Promise<void>;
  deleteWorkstream: (id: string) => Promise<void>;
  addConnection: (
    id: string,
    ws: string,
    from: string,
    to: string,
    kind: ConnKind,
    bi: boolean
  ) => Promise<void>;
  removeConnection: (id: string) => Promise<void>;
  createProfile: (id: string, name: string) => Promise<void>;
  deleteProfile: (id: string) => Promise<void>;
  assignSession: (
    sessionId: string,
    wsId: string,
    lead: boolean
  ) => Promise<void>;
  setActiveProfile: (profile: string | null) => Promise<void>;
}

const EMPTY_GROUPS: OverviewGroups = { attention: [], active: [], idle: [] };

// ---------------------------------------------------------------------------
// Helpers (mirrors marsad-viewmodel Rust logic)
// ---------------------------------------------------------------------------

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

function hostLabel(hostId: string, hosts: Host[]): string {
  return hosts.find((h) => h.id === hostId)?.label ?? hostId;
}

function stateOrder(p: Phase): number {
  const order: Record<Phase, number> = {
    Needs: 0,
    Error: 1,
    Working: 2,
    Starting: 3,
    Done: 4,
    Stale: 5,
    Detached: 6,
  };
  return order[p] ?? 99;
}

function gateKind(sv: SessionView): string | null {
  if (!sv.gate) return null;
  if ("Permission" in sv.gate) return "permission";
  if ("Plan" in sv.gate) return "plan";
  if ("Edit" in sv.gate) return "edit";
  if ("Question" in sv.gate) return "question";
  return null;
}

function gateSummary(sv: SessionView): string | null {
  if (!sv.gate) return null;
  if ("Permission" in sv.gate) return sv.gate.Permission.command;
  if ("Plan" in sv.gate) return sv.gate.Plan.summary;
  if ("Edit" in sv.gate) return sv.gate.Edit.file;
  if ("Question" in sv.gate) {
    const qs = sv.gate.Question.questions;
    if (qs.length === 0) return null;
    if (qs.length === 1) return qs[0].question;
    return `${qs.length} questions pending`;
  }
  return null;
}

function toCard(sv: SessionView, hosts: Host[]): SessionCard {
  return {
    id: sv.id,
    name: sv.name,
    state: sv.state,
    host_label: hostLabel(sv.host_id, hosts),
    project: sv.project,
    branch: sv.branch,
    gate_kind: gateKind(sv),
    gate_summary: gateSummary(sv),
    last_action: null,
    context_pct: sv.context_pct,
    tokens_label: fmtTokens(sv.tokens),
    elapsed_label: "",
    subagents: sv.subagents,
    running_subagents: sv.running_subagents,
  };
}

function computeCounts(sessions: SessionView[], _hosts: Host[]): Counts {
  const total = sessions.length;
  const needs = sessions.filter((s) => s.state === "Needs").length;
  const errors = sessions.filter((s) => s.state === "Error").length;
  const working = sessions.filter((s) => s.state === "Working").length;
  const costSum = sessions.reduce((a, s) => a + s.cost_usd, 0);
  const tokenSum = sessions.reduce((a, s) => a + s.tokens, 0);
  const seenHosts = new Set(sessions.map((s) => s.host_id));
  return {
    total,
    needs,
    errors,
    working,
    cost: `$${Math.abs(costSum).toFixed(2)}`,
    tokens: fmtTokens(tokenSum),
    host_count: seenHosts.size,
  };
}

function computeGroups(sessions: SessionView[], hosts: Host[]): OverviewGroups {
  const attention = [
    ...sessions.filter((s) => s.state === "Needs"),
    ...sessions.filter((s) => s.state === "Error"),
  ].map((s) => toCard(s, hosts));
  const active = sessions
    .filter((s) => s.state === "Working" || s.state === "Starting")
    .map((s) => toCard(s, hosts));
  const idle = sessions
    .filter((s) => s.state === "Done")
    .map((s) => toCard(s, hosts));
  return { attention, active, idle };
}

function computeSidebar(
  sessions: SessionView[],
  hosts: Host[],
  filter: Filter,
  search: string
): SessionRow[] {
  const term = search.trim().toLowerCase();
  let filtered = sessions.filter((sv) => {
    if (filter === "Needs") return sv.state === "Needs";
    if (filter === "Working") return sv.state === "Working";
    if (filter === "Error") return sv.state === "Error";
    return true;
  });
  if (term) {
    filtered = filtered.filter((sv) => {
      const hl = hostLabel(sv.host_id, hosts).toLowerCase();
      return (
        sv.name.toLowerCase().includes(term) ||
        sv.project.toLowerCase().includes(term) ||
        hl.includes(term) ||
        sv.branch.toLowerCase().includes(term)
      );
    });
  }
  const rows = filtered.map((s) => toCard(s, hosts));
  rows.sort((a, b) => stateOrder(a.state) - stateOrder(b.state));
  return rows;
}

function computeDetail(
  sessions: SessionView[],
  selectedId: string | null,
  hosts: Host[]
): SessionDetailVM | null {
  if (!selectedId) return null;
  const sv = sessions.find((s) => s.id === selectedId);
  if (!sv) return null;
  return {
    name: sv.name,
    host_label: hostLabel(sv.host_id, hosts),
    branch: sv.branch,
    model: sv.model,
    ctx_warn: sv.context_pct >= 75,
    context_pct: sv.context_pct,
    tokens_label: fmtTokens(sv.tokens),
    cost_label: `$${sv.cost_usd.toFixed(2)}`,
    elapsed_label: "",
  };
}

function diffCellBg(sig: string): string {
  if (sig === "Add") return "rgba(70,192,138,0.12)";
  if (sig === "Del") return "rgba(224,89,74,0.12)";
  return "transparent";
}

function diffCellFg(sig: string): string {
  if (sig === "Add") return "#46c08a";
  if (sig === "Del") return "#e0594a";
  return "var(--txt2)";
}

function computeGate(
  sessions: SessionView[],
  selectedId: string | null
): GateVM | null {
  if (!selectedId) return null;
  const sv = sessions.find((s) => s.id === selectedId);
  if (!sv || !sv.gate) return null;
  const g = sv.gate;
  if ("Permission" in g) {
    return {
      kind: "permission",
      icon: "⌘",
      lead: "Permission request",
      summary: `${g.Permission.command} in ${g.Permission.cwd}`,
      decision_id: g.Permission.request_id,
      is_permission: true,
      is_plan: false,
      is_edit: false,
      is_question: false,
      diff_rows: [],
    };
  }
  if ("Plan" in g) {
    return {
      kind: "plan",
      icon: "□",
      lead: "Plan for approval",
      summary: g.Plan.summary,
      decision_id: selectedId,
      is_permission: false,
      is_plan: true,
      is_edit: false,
      is_question: false,
      diff_rows: [],
    };
  }
  if ("Question" in g) {
    const qs = g.Question.questions;
    const summary =
      qs.length === 0 ? "Question" :
      qs.length === 1 ? qs[0].question :
      `${qs.length} questions`;
    return {
      kind: "question",
      icon: "❓",
      lead: "Wants your input",
      summary,
      decision_id: g.Question.request_id,
      is_permission: false,
      is_plan: false,
      is_edit: false,
      is_question: true,
      diff_rows: [],
    };
  }
  if ("Edit" in g) {
    // Build side-by-side diff rows
    const diff_rows = g.Edit.diff.map((line) => {
      const bg = diffCellBg(line.sig);
      const fg = diffCellFg(line.sig);
      const sigChar = line.sig === "Add" ? "+" : line.sig === "Del" ? "-" : " ";
      const sigColor = line.sig === "Add" ? "#46c08a" : line.sig === "Del" ? "#e0594a" : "var(--txt3)";
      return {
        l: { ln: line.sig !== "Add" ? line.ln : "", sig: sigChar, sigColor, text: line.sig !== "Add" ? line.text : "", fg, bg },
        r: { ln: line.sig !== "Del" ? line.ln : "", sig: sigChar, sigColor, text: line.sig !== "Del" ? line.text : "", fg, bg },
      };
    });
    return {
      kind: "edit",
      icon: "±",
      lead: "File edit",
      summary: g.Edit.file,
      decision_id: selectedId,
      is_permission: false,
      is_plan: false,
      is_edit: true,
      is_question: false,
      diff_rows,
    };
  }
  return null;
}

// ---------------------------------------------------------------------------
// Context export
// ---------------------------------------------------------------------------

export const VmContext = createContext<VmContextValue | null>(null);

export function useVm(): VmContextValue {
  const ctx = useContext(VmContext);
  if (!ctx) throw new Error("useVm must be used inside VmProvider");
  return ctx;
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

export interface VmProviderProps {
  children: React.ReactNode;
  /** Override initial state (used in tests to inject mock store). */
  initialState?: Partial<VmState>;
}

export function VmProvider({ children, initialState: init }: VmProviderProps) {
  // When a snapshot is injected via initialState (e.g. tests), skip all async
  // fetching and event subscription so no act() warnings fire and no network
  // calls are made.
  const hasPreloadedSnapshot = init?.snapshot != null;

  const [state, dispatch] = useReducer(reducer, {
    ...initialState,
    ...init,
  });

  // Seed from snapshot on mount — skipped when snapshot pre-injected.
  useEffect(() => {
    if (hasPreloadedSnapshot) return;
    fetchSnapshot().then((snap) => {
      dispatch({ type: "SNAPSHOT_LOADED", payload: snap });
    });
  }, [hasPreloadedSnapshot]);

  // Listen for live vm-update events — skipped when snapshot pre-injected.
  useEffect(() => {
    if (hasPreloadedSnapshot) return;
    let unlisten: (() => void) | undefined;
    listenVmUpdate((ev) => {
      dispatch({ type: "VM_UPDATE", payload: ev as VmUpdateEvent });
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      unlisten?.();
    };
  }, [hasPreloadedSnapshot]);

  // Seed the board on mount, after the snapshot load — skipped when snapshot
  // pre-injected (tests drive board loading explicitly via openBoard()/the
  // command helpers so no act() warnings fire on mount).
  useEffect(() => {
    if (hasPreloadedSnapshot) return;
    fetchBoard().then((board) => {
      dispatch({ type: "SET_BOARD", board });
    });
  }, [hasPreloadedSnapshot]);

  // Seed capabilities on mount — skipped when snapshot pre-injected (tests
  // inject `capabilities` directly via initialState so no act() warnings fire
  // on mount). Live updates arrive via VM_UPDATE's "capabilities" arm above.
  useEffect(() => {
    if (hasPreloadedSnapshot) return;
    fetchCapabilities().then((caps) => {
      dispatch({ type: "CAPABILITIES_LOADED", payload: caps });
    });
  }, [hasPreloadedSnapshot]);

  const sessions = state.snapshot?.sessions ?? [];
  const hosts = state.snapshot?.hosts ?? [];
  const workspaces = state.snapshot?.workspaces ?? [];

  // Memoised selectors.
  const counts = useMemo(() => computeCounts(sessions, hosts), [sessions, hosts]);
  const overviewGroups = useMemo(
    () => (state.snapshot ? computeGroups(sessions, hosts) : EMPTY_GROUPS),
    [sessions, hosts]
  );
  const sidebarList = useMemo(
    () => computeSidebar(sessions, hosts, state.filter, state.search),
    [sessions, hosts, state.filter, state.search]
  );
  const selectedSession = useMemo(
    () => sessions.find((s) => s.id === state.selectedId) ?? null,
    [sessions, state.selectedId]
  );
  const selectedDetail = useMemo(
    () => computeDetail(sessions, state.selectedId, hosts),
    [sessions, state.selectedId, hosts]
  );
  const selectedGate = useMemo(
    () => computeGate(sessions, state.selectedId),
    [sessions, state.selectedId]
  );
  const activeWs = useMemo(
    () => workspaces.find((w) => w.id === state.snapshot?.active_ws) ?? null,
    [workspaces, state.snapshot?.active_ws]
  );

  // ---------------------------------------------------------------------------
  // Command helpers (U-INT-2) — call the api.ts wrapper, then refresh board
  // (and graph, when a workstream is open) so screens dispatch real backend
  // operations and observe the resulting state.
  // ---------------------------------------------------------------------------

  const refreshBoard = React.useCallback(async () => {
    const board = await fetchBoard();
    dispatch({ type: "SET_BOARD", board });
  }, []);

  const refreshGraph = React.useCallback(async (wsId: string) => {
    const graph = await fetchGraph(wsId);
    dispatch({ type: "SET_GRAPH", wsId, graph });
  }, []);

  const refreshAfterMutation = React.useCallback(
    async (openWsId: string | null) => {
      await refreshBoard();
      if (openWsId) {
        await refreshGraph(openWsId);
      }
    },
    [refreshBoard, refreshGraph]
  );

  const openBoard = React.useCallback(() => {
    dispatch({ type: "OPEN_BOARD" });
    refreshBoard();
  }, [refreshBoard]);

  const openNode = React.useCallback((sessionId: string) => {
    dispatch({ type: "SELECT_SESSION", id: sessionId });
  }, []);

  const openWorkstream = React.useCallback(
    async (id: string) => {
      const board = state.board ?? (await fetchBoard());
      const card = board.all.find((c) => c.id === id);
      if (card?.is_single_node) {
        const g = await fetchGraph(id);
        const node = g?.nodes[0];
        if (node) {
          dispatch({ type: "SELECT_SESSION", id: node.id });
        }
        return;
      }
      const graph = await fetchGraph(id);
      dispatch({ type: "SET_GRAPH", wsId: id, graph });
      dispatch({ type: "OPEN_GRAPH", wsId: id });
    },
    [state.board]
  );

  const selectNode = React.useCallback((id: string | null) => {
    dispatch({ type: "SELECT_NODE", id });
  }, []);

  const createWorkstream = React.useCallback(
    async (id: string, name: string, goal: string, profile?: string | null) => {
      await apiCreateWorkstream(id, name, goal, profile);
      await refreshAfterMutation(state.wsId);
    },
    [refreshAfterMutation, state.wsId]
  );

  const deleteWorkstream = React.useCallback(
    async (id: string) => {
      await apiDeleteWorkstream(id);
      await refreshAfterMutation(state.wsId);
    },
    [refreshAfterMutation, state.wsId]
  );

  const addConnection = React.useCallback(
    async (
      id: string,
      ws: string,
      from: string,
      to: string,
      kind: ConnKind,
      bi: boolean
    ) => {
      await apiAddConnection(id, ws, from, to, kind, bi);
      await refreshAfterMutation(state.wsId);
    },
    [refreshAfterMutation, state.wsId]
  );

  const removeConnection = React.useCallback(
    async (id: string) => {
      await apiRemoveConnection(id);
      await refreshAfterMutation(state.wsId);
    },
    [refreshAfterMutation, state.wsId]
  );

  const createProfile = React.useCallback(
    async (id: string, name: string) => {
      await apiCreateProfile(id, name);
      await refreshAfterMutation(state.wsId);
    },
    [refreshAfterMutation, state.wsId]
  );

  const deleteProfile = React.useCallback(
    async (id: string) => {
      await apiDeleteProfile(id);
      await refreshAfterMutation(state.wsId);
    },
    [refreshAfterMutation, state.wsId]
  );

  const assignSession = React.useCallback(
    async (sessionId: string, wsId: string, lead: boolean) => {
      await apiAssignSession(sessionId, wsId, lead);
      await refreshAfterMutation(state.wsId);
    },
    [refreshAfterMutation, state.wsId]
  );

  const setActiveProfile = React.useCallback(
    async (profile: string | null) => {
      await apiSetActiveProfile(profile);
      dispatch({ type: "SET_ACTIVE_PROFILE", profile });
      await refreshAfterMutation(state.wsId);
    },
    [refreshAfterMutation, state.wsId]
  );

  const value: VmContextValue = {
    state,
    dispatch,
    counts,
    overviewGroups,
    sidebarList,
    selectedSession,
    selectedDetail,
    selectedGate,
    workspaces,
    hosts,
    activeWs,
    openBoard,
    openWorkstream,
    selectNode,
    openNode,
    createWorkstream,
    deleteWorkstream,
    addConnection,
    removeConnection,
    createProfile,
    deleteProfile,
    assignSession,
    setActiveProfile,
  };

  return React.createElement(VmContext.Provider, { value }, children);
}
