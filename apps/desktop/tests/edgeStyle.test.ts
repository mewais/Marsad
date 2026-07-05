/**
 * Unit tests for the pure edge-semantics mapping (colour=status, dash=type).
 * Spec: docs/design/DESIGN_HANDOFF.md §"LEVEL 2" → Edges — colour must NEVER
 * encode type, and dash must NEVER encode status.
 */
import { describe, it, expect } from "vitest";
import { edgeColor, edgeDashStyle, edgeStrokeWidth } from "../src/screens/graph/edgeStyle";

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

describe("edgeDashStyle — kind drives dash pattern, independent of status", () => {
  it("data/message is dashed 8 7 and animated (marching ants)", () => {
    const s = edgeDashStyle("data");
    expect(s.dasharray).toBe("8 7");
    expect(s.animated).toBe(true);
  });
  it("command/prompt is dotted 2 5 with round caps, not animated", () => {
    const s = edgeDashStyle("command");
    expect(s.dasharray).toBe("2 5");
    expect(s.animated).toBe(false);
    expect(s.linecap).toBe("round");
  });
  it("both is solid (no dasharray) and not animated", () => {
    const s = edgeDashStyle("both");
    expect(s.dasharray).toBeUndefined();
    expect(s.animated).toBe(false);
  });

  it("the same kind produces the same dash regardless of status (colour and dash are orthogonal)", () => {
    // The design rule is explicit: "do NOT colour-code the type — type is
    // purely dash-encoded; colour is reserved for status." Assert the dash
    // mapping function doesn't even take a status parameter, so it CANNOT
    // vary by status by construction.
    expect(edgeDashStyle.length).toBe(1);
  });
});

describe("edgeStrokeWidth", () => {
  it("'both' kind is the thickest solid line regardless of status", () => {
    expect(edgeStrokeWidth("both", "normal")).toBe(2.6);
    expect(edgeStrokeWidth("both", "needs")).toBe(2.6);
  });
  it("a 'needs' status edge (of a non-both kind) is heavier than a normal one", () => {
    expect(edgeStrokeWidth("data", "needs")).toBeGreaterThan(edgeStrokeWidth("data", "normal"));
  });
});
