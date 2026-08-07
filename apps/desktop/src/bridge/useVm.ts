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
  WaslGateView,
  WaslAuditEntry,
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
  answerWaslGate,
  capsKey,
  createWorkstream as apiCreateWorkstream,
  deleteWorkstream as apiDeleteWorkstream,
  pauseWorkstream as apiPauseWorkstream,
  resumeWorkstream as apiResumeWorkstream,
  addConnection as apiAddConnection,
  removeConnection as apiRemoveConnection,
  createProfile as apiCreateProfile,
  deleteProfile as apiDeleteProfile,
  assignSession as apiAssignSession,
  setActiveProfile as apiSetActiveProfile,
} from "./api";
import type { BoardView, Driver, GraphView } from "./workstreamTypes";

// ---------------------------------------------------------------------------
// Screen / nav state (UI only — not in AppSnapshot)
// ---------------------------------------------------------------------------

// "overview" survives here even though the Overview screen is no longer
// routed to (Workstreams-redesign SLICE A deletes it as a nav destination —
// Board is the new home). Several other test suites render `<Overview/>`
// directly to exercise session-card/cost-aggregation logic and preload this
// literal as a neutral `state.screen` value; keeping it avoids unrelated
// collateral breakage. "workspaces" is fully removed — nothing outside the
// deleted Workspaces screen/test ever depended on it.
export type Screen =
  | "overview"
  | "board"
  | "graph"
  | "session"
  | "gate"
  | "new-workstream"
  | "node-modal"
  | "settings";

// ---------------------------------------------------------------------------
// Node modal context (SLICE E) — describes WHY the node/add-agent modal is
// open: which workstream the new agent joins, and whether this is the
// "single session" fast lane (launch → straight to Level-3, no Controlled-by/
// Feeds-from section since there's nothing to wire to yet) vs. adding an
// agent to a workstream that may already have nodes.
// ---------------------------------------------------------------------------

export interface NodeModalCtx {
  wsId: string;
  wsName: string;
  /** True = the New-workstream "Single session" fast lane: launch routes
   *  straight to Level-3 and this node becomes the workstream's lead. */
  isFirstNode: boolean;
  /** True = the graph's "Build it out" design-mode "＋ Add agent": the node is
   *  authored INTO the draft workstream graph (assignSession under the ws), and
   *  the confirm/launch affordance must NOT launch a standalone session — it
   *  returns to the graph so more nodes/connections can be authored. Launch is a
   *  separate, later, explicit "Confirm & launch" action. Falsy/absent = the
   *  single-session fast lane (launch immediately, straight to Level-3). */
  designMode?: boolean;
}

// ---------------------------------------------------------------------------
// Design-mode draft workstream (DESIGN_HANDOFF.md:183) — the nodes + edges the
// user is AUTHORING in the "Build it out" design canvas BEFORE "Confirm &
// launch". Nothing here has hit the real `marsad-core` yet: a DraftNode has NO
// session id (a real id only exists post-launch — core.md:29), and a DraftEdge
// references TEMP node ids. These live in the store (not GraphScreenConnected
// component-local state) because the authoring flow navigates
// graph → node-modal → graph, which unmounts GraphScreenConnected on every add;
// component-local draft state would be wiped each time.
// ---------------------------------------------------------------------------

/** A node authored into the draft graph in design mode — no session launched
 *  yet, so it carries only the launch config the modal collected. `tempId` is a
 *  client-side placeholder id used to render the node + wire draft edges; it is
 *  resolved to a real session id during "Confirm & launch". */
export interface DraftNode {
  tempId: string;
  name: string;
  project: string;
  hostId: string;
  driver: Driver;
  model: string;
  session_mode: string;
  firstPrompt?: string;
  /** True for the first authored node — it becomes the workstream's lead. */
  isLead: boolean;
  /** W2b: resume state for wasl driver launches. `resume` enables threading;
   *  `selectedResume` is either "continue" (most-recent) or a specific session id. */
  resume?: boolean;
  selectedResume?: string;
  /** W2b: the sessions list at the time of authoring, for most-recent resolution. */
  resumeSessions?: { id: string }[];
}

/** A connection authored between two draft nodes — referencing their TEMP ids.
 *  Submitted to the backend (with real ids) only after both endpoints are
 *  launched + assigned during "Confirm & launch". */
export interface DraftEdge {
  tempId: string;
  from: string;
  to: string;
  /** `bi=false` → Directed (from drives to), `bi=true` → Consult (mutual).
   *  Direction is the only edge axis (post-#7 — no "kind"). */
  bi: boolean;
}

// ---------------------------------------------------------------------------
// Store state
// ---------------------------------------------------------------------------

/** Minimal placeholder for a session that's been selected/launched but whose
 *  real SessionView hasn't arrived from the backend yet. */
export interface PendingSession {
  id: string;
  name: string;
  project: string;
  host_id: string;
  driver?: string;
}

export interface VmState {
  snapshot: AppSnapshot | null;
  // UI-only nav
  screen: Screen;
  selectedId: string | null;
  // The active tab for the CURRENT session. Per-session so switching sessions
  // never carries the Terminal tab over (which would open a terminal you didn't
  // ask for) — each session remembers its own, defaulting to Activity.
  tab: Tab;
  tabBySession: Record<string, Tab>;
  filter: Filter;
  search: string;
  wsMenuOpen: boolean;
  // Workstream board/graph nav (U-INT-2)
  board: BoardView | null;
  wsId: string | null;
  graph: GraphView | null;
  nodeSel: string | null;
  // Workstreams-redesign nav model (SLICE A): true when the currently open
  // session (`screen === "session"`) was reached via a graph node (vs.
  // straight from the board's single-session shortcut). Drives the Level-3
  // "‹ Back" label (parent workstream name vs. "Workstreams") in a later
  // slice, and IS consulted by the header breadcrumbs' click targets here.
  fromGraph: boolean;
  activeProfile: string | null;
  // SLICE E — modals & overlays: context for the node/add-agent modal, set
  // when opened from the New-workstream "Single session" path (later also
  // from the graph's design-mode "+ Add agent", a different slice).
  nodeModalCtx: NodeModalCtx | null;
  // BUG #2 — id of the just-created empty "draft" workstream, if any. A draft
  // is a workstream created by the New-workstream flow that has not yet gained
  // a session; if the user cancels/navigates back before launching, it must be
  // discarded so no empty workstream lingers on the Board. `null` when there is
  // no pending draft (the common case).
  draftWsId: string | null;
  // Design-mode ("Build it out") draft authoring state — the nodes + edges being
  // authored client-side BEFORE "Confirm & launch". See DraftNode/DraftEdge and
  // DESIGN_HANDOFF.md:183. Empty in operate mode / when no draft is in flight.
  draftNodes: DraftNode[];
  draftEdges: DraftEdge[];
  // Optimistic placeholder for a just-launched session: set the instant we
  // SELECT_SESSION with the picked name/project (before the backend's real
  // SessionView arrives), so the header/breadcrumb show the real name and the
  // transcript shows "Loading transcript…" instead of the raw session-key +
  // "No session selected." during the launch gap. Cleared when the real
  // SessionView lands (or when a different session is selected).
  pendingSession: PendingSession | null;
  // Capabilities (U2), keyed by capsKey(driver, host_id) — dynamic-capabilities.md §3.6.
  capabilities: Record<string, CapabilitiesView>;
  // Wasl (X5) — pending handoff gates + the latest observation entries.
  // Empty/absent when no Wasl node is connected (optionality: no Wasl chrome).
  waslGates: WaslGateView[];
  waslObservations: WaslAuditEntry[];
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

type Action =
  | { type: "SNAPSHOT_LOADED"; payload: AppSnapshot }
  | { type: "VM_UPDATE"; payload: VmUpdateEvent }
  | { type: "CAPABILITIES_LOADED"; payload: CapabilitiesView[] }
  | { type: "SET_SCREEN"; screen: Screen }
  | {
      type: "SELECT_SESSION";
      id: string | null;
      /** True when navigating in from a graph node (vs. the board shortcut). */
      fromGraph?: boolean;
      /** The owning workstream, when known at the call site (board/graph). */
      wsId?: string;
      /** Optimistic placeholder details for a just-launched session, so the UI
       *  shows the real name + "loading transcript" during the launch gap. */
      optimistic?: { name: string; project: string; host_id: string; driver?: string };
    }
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
  | { type: "SET_ACTIVE_PROFILE"; profile: string | null }
  | { type: "OPEN_NEW_WORKSTREAM" }
  | { type: "OPEN_NODE_MODAL"; ctx: NodeModalCtx }
  // BUG #2 — mark / clear the just-created empty "draft" workstream (a fresh
  // workstream that has no session yet). Set right after createWorkstream in
  // the New-workstream flow; cleared once it gains a session or is discarded.
  | { type: "SET_DRAFT_WS"; wsId: string | null }
  // Design-mode draft authoring — pure client-side, no backend. ADD_DRAFT_NODE
  // appends an authored node (and, when it's not the first, auto-wires a draft
  // edge from the lead → the new node so a two-node draft carries a connection
  // that materializes at Confirm time). ADD_DRAFT_EDGE records an explicit wire.
  // CLEAR_DRAFT wipes both lists (after Confirm & launch, or on Discard).
  | { type: "ADD_DRAFT_NODE"; node: DraftNode }
  | { type: "ADD_DRAFT_EDGE"; edge: DraftEdge }
  // Remove a draft node (by temp id) + every draft edge touching it; if the
  // removed node was the lead, promote the next remaining draft node to lead.
  | { type: "REMOVE_DRAFT_NODE"; tempId: string }
  // Remove a single draft edge (by temp id).
  | { type: "REMOVE_DRAFT_EDGE"; tempId: string }
  | { type: "CLEAR_DRAFT" };

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
        // The real SessionView has arrived — retire the optimistic placeholder
        // for this id so the loading state gives way to the live view.
        const pendingSession =
          state.pendingSession?.id === ev.session.id ? null : state.pendingSession;
        return { ...state, snapshot: { ...state.snapshot, sessions }, pendingSession };
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
      if (ev.kind === "wasl_gates") {
        // Full pending-gate list (X3 Hosts-style semantics) — replace wholesale.
        return { ...state, waslGates: ev.gates };
      }
      if (ev.kind === "wasl_gate_decided") {
        // A gate resolved — drop it from the pending list (the full-list arm
        // also re-syncs, but this keeps the UI snappy on the single decision).
        return {
          ...state,
          waslGates: state.waslGates.filter(
            (g) => !(g.host_id === ev.host_id && g.gate_id === ev.gate_id)
          ),
        };
      }
      if (ev.kind === "wasl_observation") {
        // Append to the observation feed (cap to the most recent 200 entries).
        const next = [...state.waslObservations, ev.entry];
        return { ...state, waslObservations: next.slice(-200) };
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
      return {
        ...state,
        screen: action.screen,
        // Leaving the node modal drops its context so a stray re-open (e.g.
        // browser back / re-render) can't resurrect a stale workstream target.
        nodeModalCtx: action.screen === "node-modal" ? state.nodeModalCtx : null,
      };

    case "SELECT_SESSION":
      return {
        ...state,
        selectedId: action.id,
        screen: action.id ? "session" : state.screen,
        fromGraph: action.id ? !!action.fromGraph : state.fromGraph,
        wsId: action.id ? (action.wsId ?? state.wsId) : state.wsId,
        // Restore this session's remembered tab (default Activity — never carry
        // the previous session's Terminal tab over).
        tab: action.id ? (state.tabBySession[action.id] ?? "Activity") : state.tab,
        // Seed the optimistic placeholder only when the caller supplied one
        // (a fresh launch); selecting an existing session clears it.
        pendingSession:
          action.id && action.optimistic
            ? { id: action.id, ...action.optimistic }
            : null,
      };

    case "SET_TAB":
      return {
        ...state,
        tab: action.tab,
        tabBySession: state.selectedId
          ? { ...state.tabBySession, [state.selectedId]: action.tab }
          : state.tabBySession,
      };

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
        fromGraph: false,
      };

    case "OPEN_GRAPH":
      return {
        ...state,
        screen: "graph",
        wsId: action.wsId,
        nodeSel: null,
        fromGraph: false,
      };

    case "SELECT_NODE":
      return { ...state, nodeSel: action.id };

    case "SET_ACTIVE_PROFILE":
      return { ...state, activeProfile: action.profile };

    case "OPEN_NEW_WORKSTREAM":
      return { ...state, screen: "new-workstream" };

    case "OPEN_NODE_MODAL":
      return { ...state, screen: "node-modal", nodeModalCtx: action.ctx };

    case "SET_DRAFT_WS":
      return { ...state, draftWsId: action.wsId };

    case "ADD_DRAFT_NODE": {
      const draftNodes = [...state.draftNodes, action.node];
      // Auto-wire each subsequent node to the lead (the first authored node), so
      // a multi-node "Build it out" draft carries the controller/feeder graph
      // the user is composing. Both endpoints are TEMP ids — this is a pure
      // client-side draft edge; it materializes (with real ids) at Confirm time.
      let draftEdges = state.draftEdges;
      const lead = state.draftNodes[0];
      if (lead) {
        draftEdges = [
          ...state.draftEdges,
          {
            tempId: `dedge-${action.node.tempId}`,
            from: lead.tempId,
            to: action.node.tempId,
            bi: false,
          },
        ];
      }
      return { ...state, draftNodes, draftEdges };
    }

    case "ADD_DRAFT_EDGE":
      return { ...state, draftEdges: [...state.draftEdges, action.edge] };

    case "REMOVE_DRAFT_NODE": {
      const draftNodes = state.draftNodes.filter((n) => n.tempId !== action.tempId);
      // Drop every draft edge that touched the removed node (dangling wires).
      let draftEdges = state.draftEdges.filter(
        (e) => e.from !== action.tempId && e.to !== action.tempId
      );
      // If the removed node was the lead, promote the first remaining node so the
      // draft always has a lead (Confirm & launch orders lead-first).
      const removedWasLead = state.draftNodes.find((n) => n.tempId === action.tempId)?.isLead;
      if (removedWasLead && draftNodes.length > 0 && !draftNodes.some((n) => n.isLead)) {
        draftNodes[0] = { ...draftNodes[0], isLead: true };
      }
      // A fully-cleared draft also clears its now-meaningless edges.
      if (draftNodes.length === 0) draftEdges = [];
      return { ...state, draftNodes, draftEdges };
    }

    case "REMOVE_DRAFT_EDGE":
      return {
        ...state,
        draftEdges: state.draftEdges.filter((e) => e.tempId !== action.tempId),
      };

    case "CLEAR_DRAFT":
      return { ...state, draftNodes: [], draftEdges: [] };

    default:
      return state;
  }
}

// Exported under stable names for reducer-level unit tests (X5a and later).
export { reducer as vmReducer };

export const initialVmState: VmState = {
  snapshot: null,
  // Board is the home screen (Workstreams redesign SLICE A) — no more
  // Overview-as-home.
  screen: "board",
  selectedId: null,
  tab: "Activity",
  tabBySession: {},
  filter: "All",
  search: "",
  wsMenuOpen: false,
  board: null,
  wsId: null,
  graph: null,
  nodeSel: null,
  fromGraph: false,
  activeProfile: null,
  nodeModalCtx: null,
  draftWsId: null,
  draftNodes: [],
  draftEdges: [],
  pendingSession: null,
  capabilities: {},
  waslGates: [],
  waslObservations: [],
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
  /** True while the selected session is an optimistic placeholder (its real
   *  SessionView hasn't landed yet) — drives the "Loading transcript…" state. */
  selectedSessionLoading: boolean;
  workspaces: Workspace[];
  hosts: Host[];
  activeWs: Workspace | null;
  // Wasl (X5): pending handoff gates + the human-decision relay. Empty ⇒ no
  // Wasl chrome renders. `answerWasl` is ONLY called from an explicit click.
  waslGates: WaslGateView[];
  answerWasl: (
    hostId: string,
    gateId: string,
    outcome: "approved" | "denied",
    reason?: string
  ) => Promise<void>;
  // Header breadcrumbs (Workstreams redesign SLICE A) — "Marsad / <Workstream>
  // / <Session>", matching the design's crumbs logic (Marsad Workstreams.dc.html
  // lines 1125-1130): the graph's own crumb is inert (already there); a
  // session's parent-workstream crumb only appears (and is clickable, back to
  // the graph) when the workstream is multi-session; the session's own crumb
  // is always last and inert.
  crumbs: { label: string; onClick: (() => void) | null }[];
  // Workstream board/graph command helpers (U-INT-2)
  openBoard: () => void;
  openWorkstream: (id: string) => Promise<void>;
  /** Fetch + open a workstream's graph directly (skips the single-node
   *  shortcut) — used after creating an empty "Build it out" workstream and
   *  by the header's parent-workstream crumb. */
  goToGraph: (id: string) => Promise<void>;
  selectNode: (id: string | null) => void;
  openNode: (sessionId: string) => void;
  // SLICE E — modals & overlays.
  /** Open the New-workstream modal (name + goal + two-path choice). */
  openNewWorkstream: () => void;
  /** Open the node/add-agent modal for `wsId`. */
  openNodeModal: (ctx: NodeModalCtx) => void;
  createWorkstream: (
    id: string,
    name: string,
    goal: string,
    profile?: string | null
  ) => Promise<void>;
  deleteWorkstream: (id: string) => Promise<void>;
  /** BUG #2 — mark (or clear, with `null`) the pending empty "draft"
   *  workstream. Set right after the New-workstream flow creates an empty
   *  workstream; cleared once it gains a session. `openBoard` discards a still-
   *  empty draft so no orphan lingers on the Board. */
  markDraftWorkstream: (id: string | null) => void;
  /** Append an authored draft node to the design-mode draft (pure client-side —
   *  NO backend call). When it's not the first node, a draft edge from the lead
   *  is auto-wired. Returns the generated temp id. */
  addDraftNode: (cfg: Omit<DraftNode, "tempId" | "isLead">) => string;
  /** Record an explicit draft edge between two draft (temp) node ids. */
  addDraftEdge: (edge: Omit<DraftEdge, "tempId">) => void;
  /** Remove a draft node by temp id (+ any draft edges touching it). No backend
   *  call — a draft node has no real session yet. */
  removeDraftNode: (tempId: string) => void;
  /** Remove a single draft edge by temp id. No backend call. */
  removeDraftEdge: (tempId: string) => void;
  /** Clear the whole design-mode draft (nodes + edges) — no backend call. */
  clearDraft: () => void;
  /** Pause every session in a workstream (backend stops each process; they
   *  remain resumable). Refreshes board + open graph afterwards. */
  pauseWorkstream: (id: string) => Promise<void>;
  /** Resume every session in a workstream. Refreshes board + open graph. */
  resumeWorkstream: (id: string) => Promise<void>;
  addConnection: (
    id: string,
    ws: string,
    from: string,
    to: string,
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

/** Client-side temp id for a draft node (no session launched yet). Prefers
 *  `crypto.randomUUID` (present in the Tauri webview); falls back to a
 *  timestamp+counter so this never throws in an older environment or collides
 *  when two nodes are authored within the same millisecond. */
let draftIdCounter = 0;
function genTempNodeId(): string {
  draftIdCounter += 1;
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return `draft-node-${crypto.randomUUID()}`;
  }
  return `draft-node-${Date.now()}-${draftIdCounter}`;
}

function genTempEdgeId(): string {
  draftIdCounter += 1;
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return `draft-edge-${crypto.randomUUID()}`;
  }
  return `draft-edge-${Date.now()}-${draftIdCounter}`;
}

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
  // U4 cost truthfulness: the aggregate "Cost today" figure is only marked
  // estimated when EVERY session contributing a nonzero cost is itself
  // backfill-only (no live agent-reported result for any of them yet).
  const costing = sessions.filter((s) => s.cost_usd !== 0);
  const costEstimated =
    costing.length > 0 && costing.every((s) => s.cost_estimated);
  return {
    total,
    needs,
    errors,
    working,
    cost: `$${Math.abs(costSum).toFixed(2)}`,
    cost_estimated: costEstimated,
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

/** Synthesize a minimal SessionView from an optimistic pending placeholder so
 *  the session screen renders the real name (state Starting, empty transcript)
 *  during the launch gap before the backend's real SessionView arrives. */
function pendingToSessionView(p: PendingSession): SessionView {
  return {
    id: p.id,
    name: p.name,
    host_id: p.host_id,
    project: p.project,
    branch: "",
    model: "",
    state: "Starting",
    changed_files: 0,
    subagents: 0,
    running_subagents: 0,
    subagent_list: [],
    cost_usd: 0,
    tokens: 0,
    context_pct: 0,
    gate: null,
    transcript: [],
    changes: [],
    model_usage: [],
    cost_estimated: false,
    tools: [],
    mcp_servers: [],
    slash_commands: [],
    driver: p.driver,
    compactions: 0,
    last_compaction: null,
    rate_limit: null,
  };
}

function computeDetail(
  sv: SessionView | null,
  hosts: Host[]
): SessionDetailVM | null {
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
      is_edit: false,
      diff_rows: [],
    };
  }
  if ("Plan" in g) {
    return {
      kind: "plan",
      icon: "□",
      lead: "Plan for approval",
      summary: g.Plan.summary,
      decision_id: g.Plan.request_id,
      is_edit: false,
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
      is_edit: false,
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
      decision_id: g.Edit.request_id,
      is_edit: true,
      diff_rows,
    };
  }
  // X1/X2: Wasl A2A send-gate — surfaced on the owning node's SessionView.gate
  // as a Handoff variant. Uses gate_id as decision_id (connector-scope identity).
  if ("Handoff" in g) {
    return {
      kind: "handoff",
      icon: "⇄", // ⇄
      lead: `A2A send from ${g.Handoff.from}`,
      summary: g.Handoff.summary,
      decision_id: g.Handoff.gate_id,
      is_edit: false,
      diff_rows: [],
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
    ...initialVmState,
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
  //
  // Cold-launch race: the agent's startup loads persisted sessions →
  // workstreams under a lock, and this mount fetch can run BEFORE that
  // finishes, returning an empty board that then never refreshes (the live
  // Board broadcast may have fired before the reader connected). So if the
  // first fetch comes back with no workstreams, retry a few times with a short
  // backoff until the backend has populated — then live VM_UPDATEs take over.
  useEffect(() => {
    if (hasPreloadedSnapshot) return;
    let cancelled = false;
    let attempts = 0;
    const seed = () => {
      fetchBoard().then((board) => {
        if (cancelled) return;
        dispatch({ type: "SET_BOARD", board });
        // Empty board during the first ~3s of a cold launch is almost always
        // "backend still loading", not "genuinely no workstreams". Retry.
        if (board.all.length === 0 && attempts < 6) {
          attempts += 1;
          setTimeout(seed, 500);
        }
      });
    };
    seed();
    return () => {
      cancelled = true;
    };
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
  // The real session, or the optimistic placeholder while a just-launched
  // session's real SessionView is still in flight.
  const selectedSession = useMemo(() => {
    const real = sessions.find((s) => s.id === state.selectedId) ?? null;
    if (real) return real;
    if (state.pendingSession && state.pendingSession.id === state.selectedId) {
      return pendingToSessionView(state.pendingSession);
    }
    return null;
  }, [sessions, state.selectedId, state.pendingSession]);
  // True while we're showing the placeholder (real view not yet arrived) — the
  // transcript renders "Loading transcript…" instead of an empty/no-session body.
  const selectedSessionLoading = useMemo(
    () =>
      !!state.pendingSession &&
      state.pendingSession.id === state.selectedId &&
      !sessions.some((s) => s.id === state.selectedId),
    [state.pendingSession, state.selectedId, sessions]
  );
  const selectedDetail = useMemo(
    () => computeDetail(selectedSession, hosts),
    [selectedSession, hosts]
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
    // BUG #2 — if there is a pending empty draft workstream (created via the
    // New-workstream flow but never given a session) and the user is heading
    // back to the Board, discard it so it doesn't linger. Only delete when it
    // genuinely still has 0 sessions; a draft that gained a session had its
    // draft marker cleared at launch, but we double-check the board card here
    // so we never delete a real, non-empty workstream.
    const draftId = state.draftWsId;
    if (draftId) {
      const card = state.board?.all.find((c) => c.id === draftId);
      const sessionCount = card?.session_count ?? 0;
      if (sessionCount === 0) {
        void apiDeleteWorkstream(draftId);
      }
      dispatch({ type: "SET_DRAFT_WS", wsId: null });
    }
    dispatch({ type: "OPEN_BOARD" });
    refreshBoard();
  }, [refreshBoard, state.draftWsId, state.board]);

  // Called from the graph screen's node-open affordance — always arrives
  // FROM a graph, so `fromGraph: true` (drives the Level-3 Back label in a
  // later slice). `state.wsId` is already set (the graph is open), so it's
  // left untouched here.
  const openNode = React.useCallback((sessionId: string) => {
    dispatch({ type: "SELECT_SESSION", id: sessionId, fromGraph: true });
  }, []);

  // Re-fetch + open a workstream's graph directly (used by the single-node
  // shortcut's "back to graph" crumb — never by the initial board click,
  // which routes through the single/multi nav rule below).
  const goToGraph = React.useCallback(async (id: string) => {
    const graph = await fetchGraph(id);
    dispatch({ type: "SET_GRAPH", wsId: id, graph });
    dispatch({ type: "OPEN_GRAPH", wsId: id });
  }, []);

  const openWorkstream = React.useCallback(
    async (id: string) => {
      const board = state.board ?? (await fetchBoard());
      const card = board.all.find((c) => c.id === id);
      if (card?.is_single_node) {
        const g = await fetchGraph(id);
        const node = g?.nodes[0];
        if (node) {
          // Single-session workstream: skip the graph entirely (nav rule).
          // `wsId` is still recorded so the header breadcrumbs / a later
          // Back button know which workstream this session belongs to.
          dispatch({
            type: "SELECT_SESSION",
            id: node.id,
            fromGraph: false,
            wsId: id,
          });
        }
        return;
      }
      await goToGraph(id);
    },
    [state.board, goToGraph]
  );

  // Header breadcrumbs — see VmContextValue.crumbs doc comment above.
  const crumbs = useMemo(() => {
    const list: { label: string; onClick: (() => void) | null }[] = [];
    if (state.screen === "graph" && state.wsId) {
      const card = state.board?.all.find((c) => c.id === state.wsId);
      const name = card?.name ?? state.graph?.name ?? state.wsId;
      list.push({ label: name, onClick: null });
      return list;
    }
    if (state.screen === "session" && state.selectedId) {
      const sv = sessions.find((s) => s.id === state.selectedId);
      const wsIdForSession = state.wsId;
      const card = wsIdForSession
        ? state.board?.all.find((c) => c.id === wsIdForSession)
        : null;
      const isMulti = card
        ? card.session_count > 1
        : state.graph?.ws_id === wsIdForSession &&
          (state.graph?.nodes.length ?? 0) > 1;
      if (isMulti && wsIdForSession) {
        const wsName = card?.name ?? state.graph?.name ?? wsIdForSession;
        list.push({ label: wsName, onClick: () => void goToGraph(wsIdForSession) });
      }
      // During the launch gap the real session isn't in `sessions` yet — fall
      // back to the optimistic placeholder's name so the crumb never flashes the
      // raw session-key.
      const label =
        sv?.name ??
        (state.pendingSession?.id === state.selectedId
          ? state.pendingSession.name
          : state.selectedId);
      list.push({ label, onClick: null });
    }
    return list;
  }, [state.screen, state.wsId, state.selectedId, state.board, state.graph, state.pendingSession, sessions, goToGraph]);

  const selectNode = React.useCallback((id: string | null) => {
    dispatch({ type: "SELECT_NODE", id });
  }, []);

  const openNewWorkstream = React.useCallback(() => {
    dispatch({ type: "OPEN_NEW_WORKSTREAM" });
  }, []);

  const openNodeModal = React.useCallback((ctx: NodeModalCtx) => {
    dispatch({ type: "OPEN_NODE_MODAL", ctx });
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

  const markDraftWorkstream = React.useCallback((id: string | null) => {
    dispatch({ type: "SET_DRAFT_WS", wsId: id });
  }, []);

  const addDraftNode = React.useCallback(
    (cfg: Omit<DraftNode, "tempId" | "isLead">): string => {
      const tempId = genTempNodeId();
      // First authored node = effective lead; reducer reads current length via
      // the dispatched node's isLead flag, which we derive from live state here.
      const isLead = state.draftNodes.length === 0;
      dispatch({ type: "ADD_DRAFT_NODE", node: { tempId, isLead, ...cfg } });
      return tempId;
    },
    [state.draftNodes.length]
  );

  const addDraftEdge = React.useCallback((edge: Omit<DraftEdge, "tempId">) => {
    dispatch({ type: "ADD_DRAFT_EDGE", edge: { tempId: genTempEdgeId(), ...edge } });
  }, []);

  const removeDraftNode = React.useCallback((tempId: string) => {
    dispatch({ type: "REMOVE_DRAFT_NODE", tempId });
  }, []);

  const removeDraftEdge = React.useCallback((tempId: string) => {
    dispatch({ type: "REMOVE_DRAFT_EDGE", tempId });
  }, []);

  const clearDraft = React.useCallback(() => {
    dispatch({ type: "CLEAR_DRAFT" });
  }, []);

  const pauseWorkstream = React.useCallback(
    async (id: string) => {
      await apiPauseWorkstream(id);
      await refreshAfterMutation(state.wsId);
    },
    [refreshAfterMutation, state.wsId]
  );

  const resumeWorkstream = React.useCallback(
    async (id: string) => {
      await apiResumeWorkstream(id);
      await refreshAfterMutation(state.wsId);
    },
    [refreshAfterMutation, state.wsId]
  );

  const addConnection = React.useCallback(
    async (id: string, ws: string, from: string, to: string, bi: boolean) => {
      await apiAddConnection(id, ws, from, to, bi);
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

  const answerWasl = React.useCallback(
    async (
      hostId: string,
      gateId: string,
      outcome: "approved" | "denied",
      reason?: string
    ) => {
      // Relay the human's decision; optimistically drop the gate from the
      // pending list so the modal closes immediately (the authoritative
      // wasl_gate_decided event confirms it).
      await answerWaslGate({ host_id: hostId, gate_id: gateId, outcome, reason });
      dispatch({
        type: "VM_UPDATE",
        payload: {
          kind: "wasl_gate_decided",
          host_id: hostId,
          gate_id: gateId,
          outcome,
          decided_by: "operator",
          reason: reason ?? null,
        },
      });
    },
    []
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
    selectedSessionLoading,
    workspaces,
    hosts,
    activeWs,
    waslGates: state.waslGates,
    answerWasl,
    crumbs,
    openBoard,
    openWorkstream,
    goToGraph,
    selectNode,
    openNode,
    openNewWorkstream,
    openNodeModal,
    createWorkstream,
    deleteWorkstream,
    markDraftWorkstream,
    addDraftNode,
    addDraftEdge,
    removeDraftNode,
    removeDraftEdge,
    clearDraft,
    pauseWorkstream,
    resumeWorkstream,
    addConnection,
    removeConnection,
    createProfile,
    deleteProfile,
    assignSession,
    setActiveProfile,
  };

  return React.createElement(VmContext.Provider, { value }, children);
}
