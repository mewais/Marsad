/**
 * Unit tests for the pure edge-semantics mapping (post-#7).
 *
 * Direction is the only edge axis now — there is NO "kind", so there is no dash
 * encoding. The stroke is a single SOLID style; only COLOUR (by status) and
 * WIDTH (by status) vary. Spec: docs/design/DESIGN_HANDOFF.md §"LEVEL 2" → Edges.
 */
import { describe, it, expect } from "vitest";
import { edgeColor, edgeStrokeWidth } from "../src/screens/graph/edgeStyle";

describe("edgeColor — status drives colour", () => {
  it("normal status is the accent blue", () => {
    expect(edgeColor("normal")).toBe("var(--accent)");
  });
  it("consulting status is the grey txt2", () => {
    expect(edgeColor("consulting")).toBe("var(--txt2)");
  });
  it("needs status is amber", () => {
    expect(edgeColor("needs")).toBe("var(--needs)");
  });
});

describe("edgeStrokeWidth — status drives width (no kind)", () => {
  it("takes ONLY status (kind is gone) — arity 1", () => {
    expect(edgeStrokeWidth.length).toBe(1);
  });
  it("a 'needs' status edge is heavier than a normal one", () => {
    expect(edgeStrokeWidth("needs")).toBeGreaterThan(edgeStrokeWidth("normal"));
  });
  it("normal and consulting share the base width (direction is shown by arrowheads, not width)", () => {
    expect(edgeStrokeWidth("normal")).toBe(edgeStrokeWidth("consulting"));
  });
});
