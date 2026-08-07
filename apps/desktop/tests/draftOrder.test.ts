/**
 * Tests for the NOT-YET-EXISTING `src/bridge/draftOrder.ts`.
 *
 * Phase C — RED suite. `orderDraftLaunch` sorts draft nodes by dependency
 * depth (mirrors the Rust `dependency_depth` from marsad-workstream::lib.rs):
 *
 *   - Sources (depth 0 — no inbound edges) come before dependents.
 *   - Lead (isLead: true) is always first among depth-0 nodes.
 *   - Within a depth level, authored order is stable (no churn among peers).
 *   - A graph with a residual cycle falls back to authored order WITHOUT
 *     throwing — the function never throws.
 *
 * Exports expected from draftOrder.ts:
 *   DraftNode  (re-exported or compatible with useVm's DraftNode)
 *   DraftEdge  (re-exported or compatible with useVm's DraftEdge)
 *   orderDraftLaunch(nodes: DraftNode[], edges: DraftEdge[]): DraftNode[]
 */
import { describe, it, expect } from "vitest";

// RED: these imports will fail until the file is created.
import { orderDraftLaunch } from "../src/bridge/draftOrder";
import type { DraftNode, DraftEdge } from "../src/bridge/draftOrder";

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

let _idSeq = 0;
function makeNode(overrides: Partial<DraftNode> & { tempId: string }): DraftNode {
  _idSeq++;
  return {
    tempId: overrides.tempId,
    name: overrides.name ?? `node-${_idSeq}`,
    project: overrides.project ?? "/code",
    hostId: overrides.hostId ?? "local",
    driver: overrides.driver ?? "claude",
    model: overrides.model ?? "claude-sonnet-latest",
    session_mode: overrides.session_mode ?? "",
    firstPrompt: overrides.firstPrompt,
    isLead: overrides.isLead ?? false,
  };
}

function makeEdge(from: string, to: string): DraftEdge {
  return { tempId: `e-${from}-${to}`, from, to, bi: false };
}

// ---------------------------------------------------------------------------
// Empty / single-node trivial cases
// ---------------------------------------------------------------------------

describe("orderDraftLaunch — trivial inputs", () => {
  it("returns an empty array given no nodes", () => {
    expect(orderDraftLaunch([], [])).toEqual([]);
  });

  it("returns the single node for a one-node graph with no edges", () => {
    const n = makeNode({ tempId: "a", isLead: true });
    const result = orderDraftLaunch([n], []);
    expect(result).toHaveLength(1);
    expect(result[0].tempId).toBe("a");
  });
});

// ---------------------------------------------------------------------------
// Sources-before-dependents for a chain authored out of order
// ---------------------------------------------------------------------------

describe("orderDraftLaunch — sources before dependents", () => {
  it("in a A→B chain authored B-then-A, A appears before B in the output", () => {
    // Authored order: B first, A second
    const b = makeNode({ tempId: "B" });
    const a = makeNode({ tempId: "A", isLead: true });
    const edge = makeEdge("A", "B"); // A is source, B is dependent

    const result = orderDraftLaunch([b, a], [edge]);
    const ids = result.map((n) => n.tempId);
    expect(ids.indexOf("A")).toBeLessThan(ids.indexOf("B"));
  });

  it("in a A→B→C chain authored C, B, A — output is A, B, C (correct depth order)", () => {
    const c = makeNode({ tempId: "C" });
    const b = makeNode({ tempId: "B" });
    const a = makeNode({ tempId: "A", isLead: true });
    const edges = [makeEdge("A", "B"), makeEdge("B", "C")];

    const result = orderDraftLaunch([c, b, a], edges);
    const ids = result.map((n) => n.tempId);
    expect(ids.indexOf("A")).toBeLessThan(ids.indexOf("B"));
    expect(ids.indexOf("B")).toBeLessThan(ids.indexOf("C"));
  });

  it("diamond: A→{B,C}→D authored in reverse — A first, D last", () => {
    const d = makeNode({ tempId: "D" });
    const c = makeNode({ tempId: "C" });
    const b = makeNode({ tempId: "B" });
    const a = makeNode({ tempId: "A", isLead: true });
    const edges = [
      makeEdge("A", "B"),
      makeEdge("A", "C"),
      makeEdge("B", "D"),
      makeEdge("C", "D"),
    ];

    const result = orderDraftLaunch([d, c, b, a], edges);
    const ids = result.map((n) => n.tempId);
    expect(ids.indexOf("A")).toBeLessThan(ids.indexOf("B"));
    expect(ids.indexOf("A")).toBeLessThan(ids.indexOf("C"));
    expect(ids.indexOf("B")).toBeLessThan(ids.indexOf("D"));
    expect(ids.indexOf("C")).toBeLessThan(ids.indexOf("D"));
  });
});

// ---------------------------------------------------------------------------
// Lead first at depth 0
// ---------------------------------------------------------------------------

describe("orderDraftLaunch — lead first at depth 0", () => {
  it("lead node (isLead:true, depth 0) is the very first element", () => {
    const nonLead1 = makeNode({ tempId: "X" });
    const nonLead2 = makeNode({ tempId: "Y" });
    const lead = makeNode({ tempId: "L", isLead: true });
    // No edges — all depth 0; lead must come first.
    const result = orderDraftLaunch([nonLead1, lead, nonLead2], []);
    expect(result[0].tempId).toBe("L");
  });

  it("lead comes first even when authored last and there are edges to dependents", () => {
    const dep = makeNode({ tempId: "D" });
    const nonLead = makeNode({ tempId: "X" });
    const lead = makeNode({ tempId: "L", isLead: true });
    const edge = makeEdge("L", "D");

    const result = orderDraftLaunch([dep, nonLead, lead], [edge]);
    expect(result[0].tempId).toBe("L");
  });

  it("in a graph with two depth-0 nodes, the lead is first and the non-lead is second", () => {
    const nonLead = makeNode({ tempId: "X" });
    const lead = makeNode({ tempId: "L", isLead: true });
    const dep = makeNode({ tempId: "D" });
    // L and X are both depth-0 sources; D depends on L.
    const edge = makeEdge("L", "D");

    const result = orderDraftLaunch([dep, nonLead, lead], [edge]);
    const ids = result.map((n) => n.tempId);
    expect(ids[0]).toBe("L");
    // X appears before D (depth 0 vs depth 1), but after L.
    expect(ids.indexOf("X")).toBeLessThan(ids.indexOf("D"));
  });
});

// ---------------------------------------------------------------------------
// Stable within a depth level
// ---------------------------------------------------------------------------

describe("orderDraftLaunch — stable within depth", () => {
  it("two depth-0 non-lead nodes retain their authored order relative to each other", () => {
    const first = makeNode({ tempId: "first" });
    const second = makeNode({ tempId: "second" });
    const lead = makeNode({ tempId: "lead", isLead: true });
    // all depth-0, no edges
    const result = orderDraftLaunch([lead, first, second], []);
    const ids = result.map((n) => n.tempId);
    // lead is fixed at index 0; among the remaining, authored order is preserved
    expect(ids.indexOf("first")).toBeLessThan(ids.indexOf("second"));
  });

  it("two depth-1 dependents retain authored order among themselves", () => {
    const dep1 = makeNode({ tempId: "D1" });
    const dep2 = makeNode({ tempId: "D2" });
    const lead = makeNode({ tempId: "L", isLead: true });
    const edges = [makeEdge("L", "D1"), makeEdge("L", "D2")];

    // Authored order: D1 before D2
    const result = orderDraftLaunch([lead, dep1, dep2], edges);
    const ids = result.map((n) => n.tempId);
    expect(ids.indexOf("D1")).toBeLessThan(ids.indexOf("D2"));
  });
});

// ---------------------------------------------------------------------------
// Cycle fallback — authored order, no throw
// ---------------------------------------------------------------------------

describe("orderDraftLaunch — cycle fallback", () => {
  it("does not throw when there is a cycle in the draft edges", () => {
    const a = makeNode({ tempId: "A", isLead: true });
    const b = makeNode({ tempId: "B" });
    const edges = [makeEdge("A", "B"), makeEdge("B", "A")]; // A→B→A cycle

    expect(() => orderDraftLaunch([a, b], edges)).not.toThrow();
  });

  it("falls back to authored order when the draft graph has a cycle", () => {
    const a = makeNode({ tempId: "A", isLead: true });
    const b = makeNode({ tempId: "B" });
    const c = makeNode({ tempId: "C" });
    // A→B→C→A cycle
    const edges = [makeEdge("A", "B"), makeEdge("B", "C"), makeEdge("C", "A")];

    const result = orderDraftLaunch([a, b, c], edges);
    // Result must contain all nodes
    expect(result).toHaveLength(3);
    const ids = result.map((n) => n.tempId);
    expect(ids).toContain("A");
    expect(ids).toContain("B");
    expect(ids).toContain("C");
    // Authored order for a cycle: A, B, C
    expect(ids.indexOf("A")).toBeLessThan(ids.indexOf("B"));
    expect(ids.indexOf("B")).toBeLessThan(ids.indexOf("C"));
  });

  it("does not throw for an empty edges list with nodes (trivial — no cycle)", () => {
    const a = makeNode({ tempId: "A", isLead: true });
    const b = makeNode({ tempId: "B" });
    expect(() => orderDraftLaunch([a, b], [])).not.toThrow();
  });
});
