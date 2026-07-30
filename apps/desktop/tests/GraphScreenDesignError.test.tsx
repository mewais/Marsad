/**
 * Phase C — RED suite for the `designError` prop on GraphScreen.
 *
 * Target behavior (NOT YET IMPLEMENTED in src):
 *   - When `designError` prop is set to a non-empty string, GraphScreen
 *     renders a `data-testid="design-conn-error"` banner containing
 *     the message text.
 *   - When `designError` is absent (undefined) or null, the banner is
 *     NOT rendered.
 *
 * Uses the same fixture/render helpers as GraphScreen.test.tsx.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { GraphScreen } from "../src/screens/graph/GraphScreen";
import type { GraphView, GraphNode, Inspector } from "../src/bridge/workstreamTypes";

// ---------------------------------------------------------------------------
// Minimal fixture helpers (same style as GraphScreen.test.tsx)
// ---------------------------------------------------------------------------

function node(id: string, host = "host-a"): GraphNode {
  return {
    id,
    name: id,
    driver: "claude",
    host,
    state: "Working",
    status: "",
    context_pct: 0,
    is_lead: false,
    depth: 0,
  };
}

function emptyInspector(): Inspector {
  return { working: 0, needs: 0, pending_handoff: null };
}

function smallGraph(): GraphView {
  return {
    ws_id: "ws1",
    name: "Test WS",
    goal: "testing",
    nodes: [node("n1"), node("n2")],
    edges: [],
    inspector: emptyInspector(),
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("GraphScreen — designError banner (GS-C1)", () => {
  it("renders data-testid='design-conn-error' when designError prop is set", () => {
    render(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={smallGraph()}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
          designMode
          designError="connection would create a dependency cycle"
        />
      </ThemeProvider>
    );
    expect(screen.getByTestId("design-conn-error")).toBeTruthy();
  });

  it("banner text contains the designError message", () => {
    const errMsg = "connection endpoints must not be the same node";
    render(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={smallGraph()}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
          designMode
          designError={errMsg}
        />
      </ThemeProvider>
    );
    const banner = screen.getByTestId("design-conn-error");
    expect(banner.textContent).toContain(errMsg);
  });

  it("renders banner even when designMode is false (errors from real-edge rejection still show)", () => {
    render(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={smallGraph()}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
          designMode={false}
          designError="an identical connection already exists"
        />
      </ThemeProvider>
    );
    expect(screen.getByTestId("design-conn-error")).toBeTruthy();
  });

  it("does NOT render design-conn-error banner when designError prop is absent", () => {
    render(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={smallGraph()}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
          designMode
          // designError omitted
        />
      </ThemeProvider>
    );
    expect(screen.queryByTestId("design-conn-error")).toBeNull();
  });

  it("does NOT render design-conn-error banner when designError is undefined explicitly", () => {
    render(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={smallGraph()}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
          designError={undefined}
        />
      </ThemeProvider>
    );
    expect(screen.queryByTestId("design-conn-error")).toBeNull();
  });

  it("does NOT render design-conn-error banner when designError is null", () => {
    render(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={smallGraph()}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
          // cast null via any since the prop type may be string|undefined
          {...({ designError: null } as Record<string, unknown>)}
        />
      </ThemeProvider>
    );
    expect(screen.queryByTestId("design-conn-error")).toBeNull();
  });

  it("banner renders with each of the five ConnError messages without truncation", () => {
    const messages = [
      "connection endpoints must not be the same node",
      "connection endpoint is not a known node",
      "connection endpoints span more than one workstream",
      "an identical connection already exists",
      "connection would create a dependency cycle",
    ];
    for (const msg of messages) {
      const { unmount } = render(
        <ThemeProvider initialTheme="dark">
          <GraphScreen
            graph={smallGraph()}
            selectedNode={null}
            onSelectNode={vi.fn()}
            onOpenNode={vi.fn()}
            onOpenGate={vi.fn()}
            designMode
            designError={msg}
          />
        </ThemeProvider>
      );
      const banner = screen.getByTestId("design-conn-error");
      expect(banner.textContent).toContain(msg);
      unmount();
    }
  });
});
