/**
 * Workstreams redesign Wave 3a — Usage tab "Cost breakdown" (By host / By
 * node), docs/design/DESIGN_HANDOFF.md §"Usage tab":
 *
 *   "Cost breakdown — By host and By node, each a stacked bar + per-row
 *   entries: driver pill, cost, % share, sorted by cost."
 *
 * Backend (`Core::cost_breakdown` / `marsad_viewmodel::workstream_cost_breakdown`,
 * commit 92d5029) already returns rows pre-sorted by cost descending and
 * carries the U4 `estimated` flag per row/total. This suite pins the FRONTEND
 * contract: the Usage tab fetches the breakdown for the open workstream
 * (`useVm().state.wsId`) via `bridge/api.ts:fetchCostBreakdown`, renders both
 * groups, preserves the given (cost-descending) row order, and only ever
 * labels truly-estimated rows/total "~ estimated" — never the live ones.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { UsageTab } from "../src/screens/tabs/Usage";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { WorkstreamCostBreakdown } from "../src/bridge/types";

// ---------------------------------------------------------------------------
// A realistic, pre-sorted breakdown fixture — mirrors the shape
// `Core::cost_breakdown` / `workstream_cost_breakdown` returns: by_host and
// by_node each sorted cost-descending, with exactly one estimated row and a
// live (non-estimated) total.
// ---------------------------------------------------------------------------

const BREAKDOWN: WorkstreamCostBreakdown = {
  total_cost_usd: 0.75,
  total_estimated: false,
  by_host: [
    { key: "build-host", label: "build-host", driver_dots: ["claude"], cost_usd: 0.67, pct: 89.3, estimated: false },
    { key: "localhost", label: "localhost", driver_dots: ["codex"], cost_usd: 0.08, pct: 10.7, estimated: true },
  ],
  by_node: [
    { key: "s06", label: "data-ingestion-pipeline", driver_dots: ["claude"], cost_usd: 0.67, pct: 89.3, estimated: false },
    { key: "s07", label: "ci-pipeline-fix", driver_dots: ["codex"], cost_usd: 0.08, pct: 10.7, estimated: true },
  ],
};

const fetchCostBreakdownMock = vi.fn();

vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  return {
    ...original,
    fetchCostBreakdown: (wsId: string) => fetchCostBreakdownMock(wsId),
  };
});

beforeEach(() => {
  vi.clearAllMocks();
  fetchCostBreakdownMock.mockResolvedValue(BREAKDOWN);
});
afterEach(() => {
  vi.restoreAllMocks();
});

function renderUsageTab(wsId: string | null) {
  const preloaded: Partial<VmState> = {
    snapshot: MOCK_SNAPSHOT,
    screen: "session",
    selectedId: "s04",
    tab: "Usage",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    wsId,
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <UsageTab />
      </VmProvider>
    </ThemeProvider>
  );
}

describe("UsageTab Cost breakdown (Wave 3a)", () => {
  it("fetches the breakdown for the open workstream (useVm().state.wsId)", async () => {
    renderUsageTab("ws-build-pipeline");
    await screen.findByTestId("cost-breakdown");
    expect(fetchCostBreakdownMock).toHaveBeenCalledWith("ws-build-pipeline");
  });

  it("renders both By host and By node groups once the breakdown resolves", async () => {
    renderUsageTab("ws-build-pipeline");
    expect(await screen.findByTestId("cost-breakdown")).toBeTruthy();
    expect(screen.getByTestId("cost-by-host")).toBeTruthy();
    expect(screen.getByTestId("cost-by-node")).toBeTruthy();
  });

  it("preserves the given cost-descending row order (does not re-sort or reverse)", async () => {
    renderUsageTab("ws-build-pipeline");
    await screen.findByTestId("cost-breakdown");

    const hostRows = screen
      .getByTestId("cost-by-host")
      .querySelectorAll('[data-testid^="cost-row-"]');
    expect(Array.from(hostRows).map((el) => el.getAttribute("data-testid"))).toEqual([
      "cost-row-build-host",
      "cost-row-localhost",
    ]);

    const nodeRows = screen
      .getByTestId("cost-by-node")
      .querySelectorAll('[data-testid^="cost-row-"]');
    expect(Array.from(nodeRows).map((el) => el.getAttribute("data-testid"))).toEqual([
      "cost-row-s06",
      "cost-row-s07",
    ]);
  });

  it("labels only the estimated row '~ estimated', never the live row, in By host", async () => {
    renderUsageTab("ws-build-pipeline");
    await screen.findByTestId("cost-breakdown");

    const liveRow = screen.getByTestId("cost-row-build-host");
    const estimatedRow = screen.getByTestId("cost-row-localhost");
    expect(
      liveRow.textContent,
      `live row must not be marked estimated, got: "${liveRow.textContent}"`
    ).not.toMatch(/estimated/i);
    expect(
      estimatedRow.textContent,
      `estimated row must be marked, got: "${estimatedRow.textContent}"`
    ).toMatch(/estimated/i);
    expect(estimatedRow.textContent).toContain("$0.08");
    expect(estimatedRow.textContent).toContain("11%");
  });

  it("labels only the estimated row '~ estimated', never the live row, in By node", async () => {
    renderUsageTab("ws-build-pipeline");
    await screen.findByTestId("cost-breakdown");

    const liveRow = screen.getByTestId("cost-row-s06");
    const estimatedRow = screen.getByTestId("cost-row-s07");
    expect(liveRow.textContent).not.toMatch(/estimated/i);
    expect(estimatedRow.textContent).toMatch(/estimated/i);
  });

  it("labels the section total '~ estimated' ONLY when total_estimated is true", async () => {
    renderUsageTab("ws-build-pipeline");
    const total = await screen.findByTestId("cost-breakdown-total");
    expect(
      total.textContent,
      `total must not be marked estimated when total_estimated is false, got: "${total.textContent}"`
    ).not.toMatch(/estimated/i);
    expect(total.textContent).toContain("$0.75");
  });

  it("marks the total '~ estimated' when the backend reports total_estimated: true", async () => {
    fetchCostBreakdownMock.mockResolvedValue({
      ...BREAKDOWN,
      total_estimated: true,
    });
    renderUsageTab("ws-build-pipeline");
    const total = await screen.findByTestId("cost-breakdown-total");
    expect(total.textContent).toMatch(/estimated/i);
  });

  it("renders a driver pill per row carrying the row's driver(s)", async () => {
    renderUsageTab("ws-build-pipeline");
    await screen.findByTestId("cost-breakdown");
    const liveRow = screen.getByTestId("cost-row-build-host");
    const pill = liveRow.querySelector('[data-testid="cost-driver-pill"]');
    expect(pill).toBeTruthy();
    expect(pill?.getAttribute("data-driver")).toBe("claude");
  });

  it("does not render the Cost breakdown section when no workstream is open", () => {
    renderUsageTab(null);
    expect(screen.queryByTestId("cost-breakdown")).toBeNull();
    expect(fetchCostBreakdownMock).not.toHaveBeenCalled();
  });

  it("does not render the Cost breakdown section when the backend has no data for the workstream (null)", async () => {
    fetchCostBreakdownMock.mockResolvedValue(null);
    renderUsageTab("ws-unknown");
    await new Promise((r) => setTimeout(r, 0));
    expect(screen.queryByTestId("cost-breakdown")).toBeNull();
  });

  it("does not render the Cost breakdown section when the workstream has no cost rows yet (empty)", async () => {
    fetchCostBreakdownMock.mockResolvedValue({
      total_cost_usd: 0,
      total_estimated: false,
      by_host: [],
      by_node: [],
    });
    renderUsageTab("ws-empty");
    await new Promise((r) => setTimeout(r, 0));
    expect(screen.queryByTestId("cost-breakdown")).toBeNull();
  });
});
