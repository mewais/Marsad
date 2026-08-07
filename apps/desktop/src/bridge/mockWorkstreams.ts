/**
 * Mock backend for the U-INT-1 desktop bridge integration (workstream command
 * surface). Simulates `marsad-core`'s workstream/profile/connection/graph
 * state in-memory so the wired app (and Vitest) can drive the full command
 * surface without a Tauri runtime. See docs/specs/ui-bridge-integration.md §3.
 */
import type { Connection, Profile, Workstream } from "./types";
import type {
  BoardView,
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

/** A host known to the system, independent of whether a node is on it right now. */
interface MockHostStatus {
  id: string;
  label: string;
  kind: string; // "local" | "wsl" | "remote"
  status: string; // "connected" | "reconnecting" | "unreachable"
}

/** A host<->host connectivity link (Level-1 host-health strip). */
interface MockHostLink {
  a: string;
  b: string;
  status: string; // "ok" | "degraded" | "down"
  note?: string;
}

export interface MockState {
  workstreams: Workstream[];
  profiles: Profile[];
  connections: Connection[]; // marsad_workstream::Connection shape
  nodes: MockNode[]; // per-session graph nodes (seed a few)
  membership: Record<string, string>; // session_id -> ws_id
  activeProfile: string | null;
  // Level-1 board host-health strip — hosts known to the system (a superset
  // of the hosts any node currently runs on) + their pairwise link health.
  hostStatuses: MockHostStatus[];
  hostLinks: MockHostLink[];
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
        status: "npx prisma migrate deploy",
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
    // Host-health strip seed: localhost + build-host are actually running
    // nodes above; wsl-dev is a connected-but-idle host known to the system
    // (reconnecting right now) so the strip shows real status variety. The
    // localhost<->build-host link is degraded so the board's link-issue chip
    // has something real to render (design §Level 1 Host health strip).
    hostStatuses: [
      { id: "localhost", label: "localhost", kind: "local", status: "connected" },
      { id: "wsl-dev", label: "wsl-dev", kind: "wsl", status: "reconnecting" },
      { id: "build-host", label: "build-host", kind: "remote", status: "connected" },
    ],
    hostLinks: [
      {
        a: "localhost",
        b: "build-host",
        status: "degraded",
        note: "packet loss · 41%",
      },
    ],
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
  const counts: StateCounts = { working: 0, needs: 0, error: 0, done: 0, paused: 0 };
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

/**
 * Attention-card "headline" (the specific blocker) + contextual CTA label —
 * mirrors the design reference's own derivation (docs/design/"Marsad
 * Workstreams.dc.html" ~line 972-974): a pending cross-host handoff gate
 * always wins ("Review handoff"); otherwise a `needs`-state session's
 * pending command surfaces as "<who> wants to run: <command>" ("Review");
 * anything else falls back to the erroring session's status, or nothing.
 */
function headlineAndCta(
  ws: Workstream,
  nodes: MockNode[],
  headState: HeadState | null,
  gated: boolean,
): { headline?: string; cta_label?: string } {
  if (gated) {
    return {
      headline: "Cross-host handoff waiting — build artifact → test host.",
      cta_label: "Review handoff",
    };
  }
  if (headState === "needs") {
    const needsNode = nodes.find((n) => n.state === "Needs");
    if (!needsNode) return { headline: "A session is waiting on your decision.", cta_label: "Review" };
    const who = nodes.length <= 1 ? ws.name : needsNode.name;
    return { headline: `${who} wants to run: ${needsNode.status}`, cta_label: "Review" };
  }
  if (headState === "error") {
    const errNode = nodes.find((n) => n.state === "Error");
    return {
      headline: errNode ? `${errNode.name}: ${errNode.status}` : "A session errored.",
      cta_label: "Open",
    };
  }
  return { cta_label: "Open" };
}

function wsToCard(ws: Workstream): WorkstreamCard {
  const nodes = nodesForWs(ws.id);
  const headState = headStateForWs(ws.id);
  const gated = wsHasGatedConnection(ws.id);
  const needsAttention = headState === "needs" || headState === "error" || gated;
  const { headline, cta_label } = needsAttention
    ? headlineAndCta(ws, nodes, headState, gated)
    : {};
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
    headline,
    cta_label,
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

  addConnection(id: string, ws: string, from: string, to: string, bi: boolean): void {
    const conn: Connection = { id, ws, from, to, bi, has_gate: false };
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

    // Host-health strip: union of known host-statuses (seeded system hosts)
    // and any host a node is actually running on that isn't already listed
    // (e.g. a session assigned ad hoc to a host not in hostStatuses).
    const knownIds = new Set(state.hostStatuses.map((h) => h.id));
    const extraNodeHosts = Array.from(new Set(state.nodes.map((n) => n.host))).filter(
      (h) => !knownIds.has(h),
    );
    const hostChips = [
      ...state.hostStatuses.map((h) => ({
        id: h.id,
        label: h.label,
        kind: h.kind,
        status: h.status,
      })),
      ...extraNodeHosts.map((h) => ({
        id: h,
        label: h,
        kind: h === "localhost" ? "local" : "remote",
        status: "connected",
      })),
    ];
    const linkChips = state.hostLinks
      .filter((l) => l.status !== "ok")
      .map((l) => ({ a: l.a, b: l.b, status: l.status, note: l.note ?? null }));

    return {
      summary: {
        workstreams: cards.length,
        sessions: Object.keys(state.membership).length,
        hosts: hostChips.length,
      },
      host_chips: hostChips,
      link_chips: linkChips,
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
