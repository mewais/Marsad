/**
 * X5c — Wasl activity surface on the Overview: the observation feed (agent-to-
 * agent traffic) + a presence chip. Renders nothing when no Wasl node is
 * connected (optionality).
 */
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { Overview } from "../src/screens/Overview";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { WaslAuditEntry, WaslGateView } from "../src/bridge/types";

function renderOverview(
  waslObservations: WaslAuditEntry[],
  waslGates: WaslGateView[] = []
) {
  const preloaded: Partial<VmState> = {
    snapshot: MOCK_SNAPSHOT,
    screen: "overview",
    waslObservations,
    waslGates,
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <Overview />
      </VmProvider>
    </ThemeProvider>
  );
}

const entry = (seq: number, actor: string, target: string, kind: string): WaslAuditEntry => ({
  seq,
  kind,
  actor,
  targets: [target],
  summary: `${kind} ${seq}`,
});

describe("X5c — Wasl activity surface", () => {
  it("shows the presence chip and observed connections", () => {
    renderOverview([entry(1, "alpha", "beta", "consult"), entry(2, "beta", "gamma", "message")]);
    expect(screen.getByTestId("wasl-presence-chip")).toBeTruthy();
    expect(screen.getByTestId("wasl-activity").textContent).toContain("2 connections observed");
    const rows = screen.getAllByTestId("wasl-activity-row");
    expect(rows.length).toBe(2);
    expect(rows[0].textContent).toContain("beta → gamma"); // newest first
  });

  it("notes pending approvals when gates await", () => {
    const g: WaslGateView = {
      host_id: "h",
      gate_id: "g1",
      kind: "consult",
      from: "alpha",
      summary: "s",
      preview: "p",
      requested_at: "",
      deadline: "",
    };
    renderOverview([entry(1, "alpha", "beta", "consult")], [g]);
    expect(screen.getByTestId("wasl-activity").textContent).toContain("awaiting approval");
  });

  it("renders NOTHING when there is no Wasl activity or gates (optionality)", () => {
    renderOverview([]);
    expect(screen.queryByTestId("wasl-activity")).toBeNull();
  });

  it("caps the visible feed to the 6 most recent", () => {
    const many = Array.from({ length: 10 }, (_, i) => entry(i, "a", "b", "message"));
    renderOverview(many);
    expect(screen.getAllByTestId("wasl-activity-row").length).toBe(6);
  });
});
