/**
 * Tests for `src/bridge/peersForNode.ts`.
 *
 * Mirrors the Rust test cases for `peers_for_node` in
 * `crates/marsad-core/src/workstreams.rs`.
 *
 * Covered scenarios:
 *   - empty connections → empty outbound and inbound
 *   - only-outbound edges → outbound filled, inbound empty
 *   - only-inbound edges → inbound filled, outbound empty
 *   - both outbound and inbound edges
 *   - duplicate edges are de-duplicated
 *   - results are sorted ascending
 *   - self-edge (from === to === nodeId) is ignored
 *   - unrelated edges (neither endpoint is nodeId) are excluded
 */
import { describe, it, expect } from "vitest";
import { peersForNode } from "../src/bridge/peersForNode";
import type { NodePeers } from "../src/bridge/peersForNode";

// Silence TS "unused" complaint for the type import used in assertions.
const _: NodePeers = { outbound: [], inbound: [] };
void _;

// ---------------------------------------------------------------------------
// Empty graph
// ---------------------------------------------------------------------------

describe("peersForNode — empty connections", () => {
  it("returns empty outbound and inbound for empty connections", () => {
    const result = peersForNode("A", []);
    expect(result.outbound).toEqual([]);
    expect(result.inbound).toEqual([]);
  });

  it("returns empty arrays for a node that does not appear in any edge", () => {
    const result = peersForNode("X", [
      { from: "A", to: "B" },
      { from: "B", to: "C" },
    ]);
    expect(result.outbound).toEqual([]);
    expect(result.inbound).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// Only-outbound
// ---------------------------------------------------------------------------

describe("peersForNode — only-outbound edges", () => {
  it("collects outbound peers when node is only a sender", () => {
    const result = peersForNode("A", [
      { from: "A", to: "B" },
      { from: "A", to: "C" },
    ]);
    expect(result.outbound).toEqual(["B", "C"]);
    expect(result.inbound).toEqual([]);
  });

  it("returns single outbound peer for a single edge", () => {
    const result = peersForNode("A", [{ from: "A", to: "B" }]);
    expect(result.outbound).toEqual(["B"]);
    expect(result.inbound).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// Only-inbound
// ---------------------------------------------------------------------------

describe("peersForNode — only-inbound edges", () => {
  it("collects inbound peers when node is only a receiver", () => {
    const result = peersForNode("C", [
      { from: "A", to: "C" },
      { from: "B", to: "C" },
    ]);
    expect(result.outbound).toEqual([]);
    expect(result.inbound).toEqual(["A", "B"]);
  });

  it("returns single inbound peer for a single edge", () => {
    const result = peersForNode("B", [{ from: "A", to: "B" }]);
    expect(result.outbound).toEqual([]);
    expect(result.inbound).toEqual(["A"]);
  });
});

// ---------------------------------------------------------------------------
// Both outbound and inbound
// ---------------------------------------------------------------------------

describe("peersForNode — both outbound and inbound", () => {
  it("correctly splits outbound and inbound in a chain A→B→C for node B", () => {
    const result = peersForNode("B", [
      { from: "A", to: "B" },
      { from: "B", to: "C" },
    ]);
    expect(result.outbound).toEqual(["C"]);
    expect(result.inbound).toEqual(["A"]);
  });

  it("handles multiple inbound and outbound edges simultaneously", () => {
    // Node M receives from A and B, sends to X and Y.
    const result = peersForNode("M", [
      { from: "A", to: "M" },
      { from: "B", to: "M" },
      { from: "M", to: "X" },
      { from: "M", to: "Y" },
    ]);
    expect(result.outbound).toEqual(["X", "Y"]);
    expect(result.inbound).toEqual(["A", "B"]);
  });
});

// ---------------------------------------------------------------------------
// Duplicate edges de-duplicated
// ---------------------------------------------------------------------------

describe("peersForNode — duplicate edges de-duplicated", () => {
  it("de-duplicates duplicate outbound edges (same from/to repeated)", () => {
    const result = peersForNode("A", [
      { from: "A", to: "B" },
      { from: "A", to: "B" }, // duplicate
      { from: "A", to: "C" },
    ]);
    expect(result.outbound).toEqual(["B", "C"]);
  });

  it("de-duplicates duplicate inbound edges", () => {
    const result = peersForNode("C", [
      { from: "A", to: "C" },
      { from: "A", to: "C" }, // duplicate
      { from: "B", to: "C" },
    ]);
    expect(result.inbound).toEqual(["A", "B"]);
  });
});

// ---------------------------------------------------------------------------
// Sorted ascending
// ---------------------------------------------------------------------------

describe("peersForNode — results sorted ascending", () => {
  it("outbound peers are sorted ascending by id", () => {
    const result = peersForNode("A", [
      { from: "A", to: "Z" },
      { from: "A", to: "M" },
      { from: "A", to: "B" },
    ]);
    expect(result.outbound).toEqual(["B", "M", "Z"]);
  });

  it("inbound peers are sorted ascending by id", () => {
    const result = peersForNode("Z", [
      { from: "C", to: "Z" },
      { from: "A", to: "Z" },
      { from: "B", to: "Z" },
    ]);
    expect(result.inbound).toEqual(["A", "B", "C"]);
  });
});

// ---------------------------------------------------------------------------
// Self-edge ignored
// ---------------------------------------------------------------------------

describe("peersForNode — self-edge ignored", () => {
  it("ignores a self-edge (from === to === nodeId)", () => {
    const result = peersForNode("A", [
      { from: "A", to: "A" }, // self-edge — must be ignored
    ]);
    expect(result.outbound).toEqual([]);
    expect(result.inbound).toEqual([]);
  });

  it("ignores self-edge even when other real edges exist", () => {
    const result = peersForNode("A", [
      { from: "A", to: "A" }, // self-edge
      { from: "A", to: "B" },
      { from: "C", to: "A" },
    ]);
    expect(result.outbound).toEqual(["B"]);
    expect(result.inbound).toEqual(["C"]);
  });
});

// ---------------------------------------------------------------------------
// Unrelated edges excluded
// ---------------------------------------------------------------------------

describe("peersForNode — unrelated edges excluded", () => {
  it("excludes edges where neither endpoint is nodeId", () => {
    const result = peersForNode("X", [
      { from: "A", to: "B" }, // unrelated
      { from: "C", to: "D" }, // unrelated
    ]);
    expect(result.outbound).toEqual([]);
    expect(result.inbound).toEqual([]);
  });

  it("correctly ignores unrelated edges while computing real peers", () => {
    const result = peersForNode("X", [
      { from: "A", to: "B" },   // unrelated
      { from: "X", to: "Y" },   // outbound
      { from: "Z", to: "X" },   // inbound
    ]);
    expect(result.outbound).toEqual(["Y"]);
    expect(result.inbound).toEqual(["Z"]);
  });
});

// ---------------------------------------------------------------------------
// Consult (bi) edges — the reverse leg counts both ways (#7)
// ---------------------------------------------------------------------------

describe("peersForNode — consult (bi) edges", () => {
  it("a consult edge A↔B makes each an outbound AND inbound peer of the other", () => {
    const conns = [{ from: "A", to: "B", bi: true }];
    const a = peersForNode("A", conns);
    expect(a.outbound).toEqual(["B"]);
    expect(a.inbound).toEqual(["B"]);
    const b = peersForNode("B", conns);
    // B authored as the `to` of a consult edge can still initiate back to A.
    expect(b.outbound).toEqual(["A"]);
    expect(b.inbound).toEqual(["A"]);
  });

  it("a directed edge A→B does NOT give B an outbound peer (controller/worker)", () => {
    const conns = [{ from: "A", to: "B", bi: false }];
    const b = peersForNode("B", conns);
    expect(b.outbound).toEqual([]);
    expect(b.inbound).toEqual(["A"]);
  });

  it("mixes directed and consult edges for the same node", () => {
    // A→B directed, A↔C consult. A can send to B and C; only C can send back to A.
    const conns = [
      { from: "A", to: "B", bi: false },
      { from: "A", to: "C", bi: true },
    ];
    const a = peersForNode("A", conns);
    expect(a.outbound).toEqual(["B", "C"]);
    expect(a.inbound).toEqual(["C"]);
  });
});
