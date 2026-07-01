/**
 * Mock backend for the U-INT-1 desktop bridge integration (workstream command
 * surface). Simulates `marsad-core`'s workstream/profile/connection/graph
 * state in-memory so the wired app (and Vitest) can drive the full command
 * surface without a Tauri runtime. See docs/specs/ui-bridge-integration.md §3.
 */
import type { Connection, Profile, Workstream } from "./types";
import type {
  BoardView,
  ConnKind,
  Driver,
  EdgeStatus,
  GraphEdge,
  GraphNode,
  GraphView,
  HandoffSummary,
  HeadState,
  Phase,
  StateCounts,
  WorkstreamCard,
} from "./workstreamTypes";

// ---------------------------------------------------------------------------
// Node — a per-session graph node, seeded a few across ≥2 hosts.
// ---------------------------------------------------------------------------

interface MockNode {
  id: string;
  name: string;
  driver: Driver;
  host: string;
  state: Phase;
  status: string;
  context_pct: number;
  is_lead: boolean;
  depth: number;
}

export interface MockState {
  workstreams: Workstream[];
  profiles: Profile[];
  connections: Connection[]; // marsad_workstream::Connection shape
  nodes: MockNode[]; // per-session graph nodes (seed a few)
  membership: Record<string, string>; // session_id -> ws_id
  activeProfile: string | null;
}

// ---------------------------------------------------------------------------
// Seed
// ---------------------------------------------------------------------------

function seedState(): MockState {
  return {
    workstreams: [
      {
        id: "ws-auth-refactor",
        name: "Refactor auth middleware",
        goal: "Migrate session auth to the new middleware stack",
        profile: null,
      },
      {
        id: "ws-build-pipeline",
        name: "Build pipeline handoff",
        goal: "Cross-host handoff waiting — build artifact → test host",
        profile: null,
      },
    ],
    profiles: [{ id: "prof-eng", name: "Engineering" }],
    connections: [
      {
        id: "conn-seed-1",
        ws: "ws-build-pipeline",
        from: "s07",
        to: "s06",
        kind: "both",
        bi: false,
        has_gate: true,
      },
    ],
    nodes: [
      {
        id: "s01",
        name: "api-auth-refactor",
        driver: "claude",
        host: "localhost",
        state: "Needs",
        status: "needs",
        context_pct: 42,
        is_lead: true,
        depth: 0,
      },
      {
        id: "s07",
        name: "ci-pipeline-fix",
        driver: "codex",
        host: "localhost",
        state: "Error",
        status: "error",
        context_pct: 65,
        is_lead: true,
        depth: 0,
      },
      {
        id: "s06",
        name: "data-ingestion-pipeline",
        driver: "claude",
        host: "build-host",
        state: "Done",
        status: "done",
        context_pct: 30,
        is_lead: false,
        depth: 1,
      },
    ],
    membership: {
      "s01": "ws-auth-refactor",
      "s07": "ws-build-pipeline",
      "s06": "ws-build-pipeline",
    },
    activeProfile: null,
  };
}

let state: MockState = seedState();

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function nodesForWs(wsId: string): MockNode[] {
  const sessionIds = Object.entries(state.membership)
    .filter(([, ws]) => ws === wsId)
    .map(([sid]) => sid);
  return state.nodes.filter((n) => sessionIds.includes(n.id));
}

function connectionsForWs(wsId: string): Connection[] {
  return state.connections.filter((c) => c.ws === wsId);
}

function edgeStatusFor(conn: Connection): EdgeStatus {
  if (conn.has_gate) return "needs";
  if (conn.bi) return "consulting";
  return "normal";
}

function nodeToGraphNode(n: MockNode): GraphNode {
  return {
    id: n.id,
    name: n.name,
    driver: n.driver,
    host: n.host,
    state: n.state,
    status: n.status,
    context_pct: n.context_pct,
    is_lead: n.is_lead,
    depth: n.depth,
  };
}

function connToGraphEdge(c: Connection): GraphEdge {
  return {
    id: c.id,
    from: c.from,
    to: c.to,
    kind: c.kind as ConnKind,
    bi: c.bi,
    status: edgeStatusFor(c),
    has_gate: !!c.has_gate,
  };
}

function headStateForWs(wsId: string): HeadState | null {
  const nodes = nodesForWs(wsId);
  if (nodes.length === 0) return null;
  // Priority: needs > error > working > done (first match across nodes).
  const priority: Phase[] = ["Needs", "Error", "Working", "Starting", "Stale"];
  for (const p of priority) {
    if (nodes.some((n) => n.state === p)) {
      return p === "Needs"
        ? "needs"
        : p === "Error"
          ? "error"
          : p === "Working" || p === "Starting" || p === "Stale"
            ? "working"
            : "done";
    }
  }
  if (nodes.every((n) => n.state === "Done")) return "done";
  return "working";
}

function countsForWs(wsId: string): StateCounts {
  const nodes = nodesForWs(wsId);
  const counts: StateCounts = { working: 0, needs: 0, error: 0, done: 0 };
  for (const n of nodes) {
    if (n.state === "Needs") counts.needs++;
    else if (n.state === "Error") counts.error++;
    else if (n.state === "Done") counts.done++;
    else counts.working++;
  }
  return counts;
}

function wsHasGatedConnection(wsId: string): boolean {
  return connectionsForWs(wsId).some((c) => c.has_gate);
}

function wsToCard(ws: Workstream): WorkstreamCard {
  const nodes = nodesForWs(ws.id);
  const headState = headStateForWs(ws.id);
  const gated = wsHasGatedConnection(ws.id);
  const needsAttention = headState === "needs" || headState === "error" || gated;
  return {
    id: ws.id,
    name: ws.name,
    goal: ws.goal,
    head_state: headState,
    driver_dots: nodes.map((n) => n.driver),
    host_chips: Array.from(new Set(nodes.map((n) => n.host))),
    session_count: nodes.length,
    counts: countsForWs(ws.id),
    needs_attention: needsAttention,
    is_single_node: nodes.length <= 1,
  };
}

function firstGatedHandoff(wsId: string): HandoffSummary | null {
  // FIRST gated connection wins — matching marsad-viewmodel::graph_view
  // (docs/specs/viewmodel-ext.md), so the mock faithfully mirrors real core.
  const gatedConns = connectionsForWs(wsId).filter((c) => c.has_gate);
  const gated = gatedConns[0];
  if (!gated) return null;
  return {
    conn_id: gated.id,
    from: gated.from,
    to: gated.to,
    payload: "handoff payload",
    risk: "medium",
  };
}

// ---------------------------------------------------------------------------
// Public mock backend surface
// ---------------------------------------------------------------------------

export const mockWorkstreams = {
  reset(): void {
    state = seedState();
  },

  state(): MockState {
    return state;
  },

  createWorkstream(
    id: string,
    name: string,
    goal: string,
    profile?: string | null,
  ): void {
    const existing = state.workstreams.find((w) => w.id === id);
    if (existing) {
      existing.name = name;
      existing.goal = goal;
      existing.profile = profile ?? null;
      return;
    }
    state.workstreams.push({ id, name, goal, profile: profile ?? null });
  },

  deleteWorkstream(id: string): void {
    state.workstreams = state.workstreams.filter((w) => w.id !== id);
    state.connections = state.connections.filter((c) => c.ws !== id);
    for (const [sessionId, wsId] of Object.entries(state.membership)) {
      if (wsId === id) delete state.membership[sessionId];
    }
  },

  addConnection(
    id: string,
    ws: string,
    from: string,
    to: string,
    kind: ConnKind,
    bi: boolean,
  ): void {
    const hasGate = kind === "both";
    const conn: Connection = { id, ws, from, to, kind, bi, has_gate: hasGate };
    const idx = state.connections.findIndex((c) => c.id === id);
    if (idx >= 0) state.connections[idx] = conn;
    else state.connections.push(conn);
  },

  removeConnection(id: string): void {
    state.connections = state.connections.filter((c) => c.id !== id);
  },

  createProfile(id: string, name: string): void {
    const existing = state.profiles.find((p) => p.id === id);
    if (existing) {
      existing.name = name;
      return;
    }
    state.profiles.push({ id, name });
  },

  deleteProfile(id: string): void {
    state.profiles = state.profiles.filter((p) => p.id !== id);
    for (const ws of state.workstreams) {
      if (ws.profile === id) ws.profile = null;
    }
    if (state.activeProfile === id) state.activeProfile = null;
  },

  assignSession(sessionId: string, wsId: string, lead: boolean): void {
    state.membership[sessionId] = wsId;
    let node = state.nodes.find((n) => n.id === sessionId);
    if (!node) {
      node = {
        id: sessionId,
        name: sessionId,
        driver: "claude",
        host: "localhost",
        state: "Working",
        status: "working",
        context_pct: 0,
        is_lead: lead,
        depth: 0,
      };
      state.nodes.push(node);
    } else {
      node.is_lead = lead;
    }
  },

  setActiveProfile(profile?: string | null): void {
    state.activeProfile = profile ?? null;
  },

  board(): BoardView {
    const filtered = state.workstreams.filter(
      (w) => !state.activeProfile || w.profile === state.activeProfile,
    );
    const cards = filtered.map(wsToCard);
    const attention = cards.filter((c) => c.needs_attention);
    const hostsSet = new Set(state.nodes.map((n) => n.host));
    return {
      summary: {
        workstreams: cards.length,
        sessions: Object.keys(state.membership).length,
        hosts: hostsSet.size,
      },
      host_chips: Array.from(hostsSet).map((h) => ({
        id: h,
        label: h,
        kind: h === "localhost" ? "local" : "remote",
        status: "connected",
      })),
      link_chips: [],
      attention,
      all: cards,
    };
  },

  graph(wsId: string): GraphView | null {
    const ws = state.workstreams.find((w) => w.id === wsId);
    if (!ws) return null;
    const nodes = nodesForWs(wsId).map(nodeToGraphNode);
    const edges = connectionsForWs(wsId).map(connToGraphEdge);
    const counts = countsForWs(wsId);
    return {
      ws_id: ws.id,
      name: ws.name,
      goal: ws.goal,
      nodes,
      edges,
      inspector: {
        working: counts.working,
        needs: counts.needs,
        pending_handoff: firstGatedHandoff(wsId),
      },
    };
  },
};
