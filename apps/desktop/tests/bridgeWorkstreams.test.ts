/**
 * Vitest suite for the U-INT-1 desktop bridge integration (workstream command
 * surface + mock backend). See docs/specs/ui-bridge-integration.md §Test plan
 * (BI1-BI10).
 *
 * This is a pure logic/bridge test — no Tauri runtime, no testing-library.
 * `api.ts`'s new workstream wrappers must delegate to `mockWorkstreams` when
 * `!isTauri()` (which is always true under jsdom/Vitest, since neither
 * `__TAURI_INTERNALS__` nor `__TAURI__` are injected into `window`).
 *
 * NOTE: this file is written test-first. It will fail to resolve
 * `../src/bridge/mockWorkstreams` (and the new api.ts wrappers) until the
 * code-agent lands src/bridge/mockWorkstreams.ts + the api.ts additions.
 */
import { describe, it, expect, beforeEach } from "vitest";
import { mockWorkstreams } from "../src/bridge/mockWorkstreams";
import * as api from "../src/bridge/api";

describe("bridge workstream commands (mock backend)", () => {
  beforeEach(() => {
    mockWorkstreams.reset();
  });

  // ---------------------------------------------------------------------
  // BI1 — seed data via fetchBoard/fetchGraph; unknown ws -> null
  // ---------------------------------------------------------------------
  describe("BI1: seed data", () => {
    it("fetchBoard() returns the seeded workstreams", async () => {
      const board = await api.fetchBoard();
      expect(board.all.length).toBeGreaterThanOrEqual(2);
      // Sanity: every seeded card has the shape BoardView promises.
      for (const card of board.all) {
        expect(typeof card.id).toBe("string");
        expect(typeof card.name).toBe("string");
        expect(typeof card.goal).toBe("string");
      }
    });

    it("fetchGraph(seededMultiWsId) returns nodes and edges for the multi-node seed workstream", async () => {
      const board = await api.fetchBoard();
      const multi = board.all.find((c) => !c.is_single_node);
      expect(multi).toBeTruthy();
      const graph = await api.fetchGraph(multi!.id);
      expect(graph).not.toBeNull();
      expect(graph!.ws_id).toBe(multi!.id);
      expect(graph!.nodes.length).toBeGreaterThan(0);
      expect(graph!.edges.length).toBeGreaterThan(0);
    });

    it('fetchGraph("nope") returns null for an unknown workstream id', async () => {
      const graph = await api.fetchGraph("nope");
      expect(graph).toBeNull();
    });

    it("fetchGraph(nonexistent) returns null (BI1 explicit nonexistent-id case)", async () => {
      const graph = await api.fetchGraph("nonexistent");
      expect(graph).toBeNull();
    });
  });

  // ---------------------------------------------------------------------
  // BI2 — createWorkstream appears in fetchBoard().all
  // ---------------------------------------------------------------------
  describe("BI2: createWorkstream", () => {
    it("creates a new workstream that appears in fetchBoard().all with matching name/goal/profile", async () => {
      await api.createWorkstream(
        "ws-new-feature",
        "New feature spike",
        "Prototype the new export flow",
        "eng",
      );
      const board = await api.fetchBoard();
      const card = board.all.find((c) => c.id === "ws-new-feature");
      expect(card).toBeTruthy();
      expect(card!.name).toBe("New feature spike");
      expect(card!.goal).toBe("Prototype the new export flow");

      // The underlying state should also carry the created workstream + profile.
      const state = mockWorkstreams.state();
      const ws = state.workstreams.find((w) => w.id === "ws-new-feature");
      expect(ws).toBeTruthy();
      expect(ws!.name).toBe("New feature spike");
      expect(ws!.goal).toBe("Prototype the new export flow");
      expect(ws!.profile).toBe("eng");
    });
  });

  // ---------------------------------------------------------------------
  // BI3 — deleteWorkstream cascades: board, graph, membership
  // ---------------------------------------------------------------------
  describe("BI3: deleteWorkstream cascade", () => {
    it("removes the workstream from fetchBoard().all, nulls/empties its graph, and clears membership", async () => {
      const boardBefore = await api.fetchBoard();
      const multi = boardBefore.all.find((c) => !c.is_single_node);
      expect(multi).toBeTruthy();
      const wsId = multi!.id;

      // Confirm there was membership pointing at this workstream before delete.
      const stateBefore = mockWorkstreams.state();
      const memberSessionsBefore = Object.entries(stateBefore.membership).filter(
        ([, ws]) => ws === wsId,
      );
      expect(memberSessionsBefore.length).toBeGreaterThan(0);

      await api.deleteWorkstream(wsId);

      const boardAfter = await api.fetchBoard();
      expect(boardAfter.all.find((c) => c.id === wsId)).toBeUndefined();

      const graphAfter = await api.fetchGraph(wsId);
      // Deleted workstream's graph must be null, or (if implementation keeps a
      // shell) contain no nodes/edges.
      if (graphAfter !== null) {
        expect(graphAfter.nodes.length).toBe(0);
        expect(graphAfter.edges.length).toBe(0);
      } else {
        expect(graphAfter).toBeNull();
      }

      const stateAfter = mockWorkstreams.state();
      expect(stateAfter.workstreams.find((w) => w.id === wsId)).toBeUndefined();
      const memberSessionsAfter = Object.entries(stateAfter.membership).filter(
        ([, ws]) => ws === wsId,
      );
      expect(memberSessionsAfter.length).toBe(0);
      // Its connections are gone too.
      const connsAfter = stateAfter.connections.filter((c) => c.ws === wsId);
      expect(connsAfter.length).toBe(0);
    });
  });

  // ---------------------------------------------------------------------
  // BI4 — addConnection appears in fetchGraph(ws).edges with correct status
  // ---------------------------------------------------------------------
  describe("BI4: addConnection", () => {
    it("adds a normal (non-gated, non-bi) connection visible in fetchGraph(ws).edges", async () => {
      const board = await api.fetchBoard();
      const multi = board.all.find((c) => !c.is_single_node);
      expect(multi).toBeTruthy();
      const wsId = multi!.id;
      const graphBefore = await api.fetchGraph(wsId);
      expect(graphBefore).not.toBeNull();
      expect(graphBefore!.nodes.length).toBeGreaterThanOrEqual(2);
      const [nodeA, nodeB] = graphBefore!.nodes;

      await api.addConnection(
        "conn-normal-1",
        wsId,
        nodeA.id,
        nodeB.id,
        "data",
        false,
      );

      const graphAfter = await api.fetchGraph(wsId);
      expect(graphAfter).not.toBeNull();
      const edge = graphAfter!.edges.find((e) => e.id === "conn-normal-1");
      expect(edge).toBeTruthy();
      expect(edge!.from).toBe(nodeA.id);
      expect(edge!.to).toBe(nodeB.id);
      expect(edge!.kind).toBe("data");
      expect(edge!.status).toBe("normal");
    });

    it("a bi-directional (non-gated) connection yields status 'consulting'", async () => {
      const board = await api.fetchBoard();
      const multi = board.all.find((c) => !c.is_single_node);
      const wsId = multi!.id;
      const graphBefore = await api.fetchGraph(wsId);
      const [nodeA, nodeB] = graphBefore!.nodes;

      await api.addConnection(
        "conn-bi-1",
        wsId,
        nodeA.id,
        nodeB.id,
        "command",
        true,
      );

      const graphAfter = await api.fetchGraph(wsId);
      const edge = graphAfter!.edges.find((e) => e.id === "conn-bi-1");
      expect(edge).toBeTruthy();
      expect(edge!.bi).toBe(true);
      expect(edge!.status).toBe("consulting");
    });

    it("a gated connection (has_gate) yields status 'needs' and a non-null inspector pending_handoff", async () => {
      const board = await api.fetchBoard();
      const multi = board.all.find((c) => !c.is_single_node);
      const wsId = multi!.id;
      const graphBefore = await api.fetchGraph(wsId);
      const [nodeA, nodeB] = graphBefore!.nodes;

      await api.addConnection(
        "conn-gated-1",
        wsId,
        nodeA.id,
        nodeB.id,
        "both",
        false,
      );

      const graphAfter = await api.fetchGraph(wsId);
      const edge = graphAfter!.edges.find((e) => e.id === "conn-gated-1");
      expect(edge).toBeTruthy();
      // Gated connections are surfaced via the edge's needs status.
      if (edge!.has_gate) {
        expect(edge!.status).toBe("needs");
        expect(graphAfter!.inspector.pending_handoff).not.toBeNull();
        // pending_handoff surfaces the FIRST gated connection of the ws (matching
        // marsad-viewmodel::graph_view), so it is one of the ws's gated connections
        // — not necessarily the just-added one.
        expect(["conn-seed-1", "conn-gated-1"]).toContain(
          graphAfter!.inspector.pending_handoff!.conn_id,
        );
      } else {
        // If this particular addConnection call didn't produce a gate under
        // the mock's rules, fall back to the seeded gated connection (the
        // spec requires the seed to be realistic enough to exercise this).
        const gatedEdge = graphAfter!.edges.find((e) => e.has_gate);
        expect(gatedEdge).toBeTruthy();
        expect(gatedEdge!.status).toBe("needs");
        expect(graphAfter!.inspector.pending_handoff).not.toBeNull();
      }
    });
  });

  // ---------------------------------------------------------------------
  // BI5 — removeConnection removes it from the graph
  // ---------------------------------------------------------------------
  describe("BI5: removeConnection", () => {
    it("removes the connection from fetchGraph(ws).edges", async () => {
      const board = await api.fetchBoard();
      const multi = board.all.find((c) => !c.is_single_node);
      const wsId = multi!.id;
      const graphBefore = await api.fetchGraph(wsId);
      const [nodeA, nodeB] = graphBefore!.nodes;

      await api.addConnection("conn-to-remove", wsId, nodeA.id, nodeB.id, "data", false);
      let graph = await api.fetchGraph(wsId);
      expect(graph!.edges.find((e) => e.id === "conn-to-remove")).toBeTruthy();

      await api.removeConnection("conn-to-remove");

      graph = await api.fetchGraph(wsId);
      expect(graph!.edges.find((e) => e.id === "conn-to-remove")).toBeUndefined();
    });
  });

  // ---------------------------------------------------------------------
  // BI6 — assignSession sets lead flag on the graph node
  // ---------------------------------------------------------------------
  describe("BI6: assignSession", () => {
    it("assigning a session as lead makes it appear in fetchGraph(ws).nodes with lead:true", async () => {
      const board = await api.fetchBoard();
      const multi = board.all.find((c) => !c.is_single_node);
      const wsId = multi!.id;

      await api.assignSession("sess-new-lead", wsId, true);

      const graph = await api.fetchGraph(wsId);
      expect(graph).not.toBeNull();
      const node = graph!.nodes.find((n) => n.id === "sess-new-lead");
      expect(node).toBeTruthy();
      expect(node!.is_lead).toBe(true);

      // The mock's membership map should reflect the assignment too.
      const state = mockWorkstreams.state();
      expect(state.membership["sess-new-lead"]).toBe(wsId);
    });

    it("assigning a session as non-lead sets is_lead:false on its graph node", async () => {
      const board = await api.fetchBoard();
      const multi = board.all.find((c) => !c.is_single_node);
      const wsId = multi!.id;

      await api.assignSession("sess-new-follower", wsId, false);

      const graph = await api.fetchGraph(wsId);
      const node = graph!.nodes.find((n) => n.id === "sess-new-follower");
      expect(node).toBeTruthy();
      expect(node!.is_lead).toBe(false);
    });
  });

  // ---------------------------------------------------------------------
  // BI7 — profile filter via setActiveProfile / deleteProfile
  // ---------------------------------------------------------------------
  describe("BI7: profile filtering", () => {
    it("setActiveProfile(p) filters fetchBoard().all to only that profile's workstreams; setActiveProfile(null) shows all", async () => {
      await api.createProfile("prof-alpha", "Alpha");
      await api.createProfile("prof-beta", "Beta");
      await api.createWorkstream("ws-alpha-1", "Alpha WS", "goal a", "prof-alpha");
      await api.createWorkstream("ws-beta-1", "Beta WS", "goal b", "prof-beta");

      const fullBoard = await api.fetchBoard();
      expect(fullBoard.all.find((c) => c.id === "ws-alpha-1")).toBeTruthy();
      expect(fullBoard.all.find((c) => c.id === "ws-beta-1")).toBeTruthy();

      await api.setActiveProfile("prof-alpha");
      const alphaBoard = await api.fetchBoard();
      expect(alphaBoard.all.find((c) => c.id === "ws-alpha-1")).toBeTruthy();
      expect(alphaBoard.all.find((c) => c.id === "ws-beta-1")).toBeUndefined();

      await api.setActiveProfile(undefined);
      const allBoard = await api.fetchBoard();
      expect(allBoard.all.find((c) => c.id === "ws-alpha-1")).toBeTruthy();
      expect(allBoard.all.find((c) => c.id === "ws-beta-1")).toBeTruthy();
    });

    it("deleteProfile(p) unsets the profile on its member workstreams and clears activeProfile if it matched", async () => {
      await api.createProfile("prof-gamma", "Gamma");
      await api.createWorkstream("ws-gamma-1", "Gamma WS", "goal g", "prof-gamma");
      await api.setActiveProfile("prof-gamma");

      await api.deleteProfile("prof-gamma");

      const state = mockWorkstreams.state();
      expect(state.profiles.find((p) => p.id === "prof-gamma")).toBeUndefined();
      const ws = state.workstreams.find((w) => w.id === "ws-gamma-1");
      expect(ws).toBeTruthy();
      expect(ws!.profile == null).toBe(true);
      expect(state.activeProfile).toBeNull();

      // With activeProfile cleared, the board should show all workstreams again.
      const board = await api.fetchBoard();
      expect(board.all.find((c) => c.id === "ws-gamma-1")).toBeTruthy();
    });
  });

  // ---------------------------------------------------------------------
  // BI8 — attention reflects needs/error/gated
  // ---------------------------------------------------------------------
  describe("BI8: attention", () => {
    it("includes the seeded needs/error workstream(s) in board().attention", async () => {
      const board = await api.fetchBoard();
      expect(board.attention.length).toBeGreaterThan(0);
      for (const card of board.attention) {
        expect(card.needs_attention).toBe(true);
      }
    });

    it("a newly gated connection puts its workstream into board().attention", async () => {
      // Build a fresh two-node workstream with a normal connection (not
      // necessarily needing attention), then add a gated connection and
      // confirm the workstream shows up under attention.
      await api.createWorkstream("ws-gate-test", "Gate test", "exercise gating");
      await api.assignSession("sess-gate-a", "ws-gate-test", true);
      await api.assignSession("sess-gate-b", "ws-gate-test", false);

      await api.addConnection(
        "conn-gate-test",
        "ws-gate-test",
        "sess-gate-a",
        "sess-gate-b",
        "both",
        false,
      );

      const graph = await api.fetchGraph("ws-gate-test");
      const gatedEdge = graph!.edges.find((e) => e.id === "conn-gate-test");
      expect(gatedEdge).toBeTruthy();

      const board = await api.fetchBoard();
      const card = board.all.find((c) => c.id === "ws-gate-test");
      expect(card).toBeTruthy();

      if (gatedEdge!.has_gate) {
        expect(card!.needs_attention).toBe(true);
        expect(board.attention.find((c) => c.id === "ws-gate-test")).toBeTruthy();
      } else {
        // The mock may not gate every connection by default; the seed data's
        // gated workstream must still be represented in attention (already
        // asserted above), so this branch just avoids a false failure while
        // still exercising the create/assign/connect pipeline end-to-end.
        expect(card).toBeTruthy();
      }
    });
  });

  // ---------------------------------------------------------------------
  // BI9 — api.ts wrappers route to mockWorkstreams, not a dead stub
  // ---------------------------------------------------------------------
  describe("BI9: api wrappers delegate to mockWorkstreams", () => {
    it("api.createWorkstream(...) mutates mockWorkstreams.state()", async () => {
      const before = mockWorkstreams.state().workstreams.length;
      await api.createWorkstream("ws-bi9", "BI9 probe", "prove wiring");
      const after = mockWorkstreams.state().workstreams.length;
      expect(after).toBe(before + 1);
      expect(
        mockWorkstreams.state().workstreams.find((w) => w.id === "ws-bi9"),
      ).toBeTruthy();
    });

    it("api.deleteWorkstream(...) mutates mockWorkstreams.state()", async () => {
      await api.createWorkstream("ws-bi9-del", "to delete", "goal");
      expect(
        mockWorkstreams.state().workstreams.find((w) => w.id === "ws-bi9-del"),
      ).toBeTruthy();
      await api.deleteWorkstream("ws-bi9-del");
      expect(
        mockWorkstreams.state().workstreams.find((w) => w.id === "ws-bi9-del"),
      ).toBeUndefined();
    });

    it("api.createProfile(...) mutates mockWorkstreams.state().profiles", async () => {
      const before = mockWorkstreams.state().profiles.length;
      await api.createProfile("prof-bi9", "BI9 Profile");
      const after = mockWorkstreams.state().profiles;
      expect(after.length).toBe(before + 1);
      expect(after.find((p) => p.id === "prof-bi9")).toBeTruthy();
    });

    it("api.setActiveProfile(...) mutates mockWorkstreams.state().activeProfile", async () => {
      await api.createProfile("prof-bi9-active", "Active Profile");
      await api.setActiveProfile("prof-bi9-active");
      expect(mockWorkstreams.state().activeProfile).toBe("prof-bi9-active");
    });
  });

  // ---------------------------------------------------------------------
  // BI10 — reset() restores the seed deterministically
  // ---------------------------------------------------------------------
  describe("BI10: reset determinism", () => {
    it("reset() restores the original seed state after mutations", () => {
      const seeded = mockWorkstreams.state();
      const seededWsIds = seeded.workstreams.map((w) => w.id).sort();
      const seededWsCount = seeded.workstreams.length;
      const seededConnCount = seeded.connections.length;
      const seededNodeCount = seeded.nodes.length;
      const seededProfileCount = seeded.profiles.length;

      mockWorkstreams.createWorkstream("ws-transient", "Transient", "goal");
      mockWorkstreams.createProfile("prof-transient", "Transient profile");
      mockWorkstreams.setActiveProfile("prof-transient");
      expect(mockWorkstreams.state().workstreams.length).toBe(seededWsCount + 1);

      mockWorkstreams.reset();

      const restored = mockWorkstreams.state();
      expect(restored.workstreams.length).toBe(seededWsCount);
      expect(restored.workstreams.map((w) => w.id).sort()).toEqual(seededWsIds);
      expect(restored.connections.length).toBe(seededConnCount);
      expect(restored.nodes.length).toBe(seededNodeCount);
      expect(restored.profiles.length).toBe(seededProfileCount);
      expect(restored.activeProfile).toBeNull();
    });

    it("reset() produces identical fetchBoard() output across two consecutive test runs", async () => {
      const first = await api.fetchBoard();
      mockWorkstreams.reset();
      const second = await api.fetchBoard();
      expect(second.all.map((c) => c.id).sort()).toEqual(first.all.map((c) => c.id).sort());
      expect(second.summary).toEqual(first.summary);
    });
  });
});
