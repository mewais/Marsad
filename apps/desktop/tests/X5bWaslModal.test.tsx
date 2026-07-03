/**
 * X5b — the real Wasl handoff-approval modal. Marsad surfaces a pending Wasl
 * gate and RELAYS the human's Approve/Deny; it never auto-approves and emits
 * no decision without a click. Absent when no gate is pending (optionality).
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { AppShell } from "../src/components/AppShell";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { WaslGateView } from "../src/bridge/types";

const answerWaslGate = vi.fn(async (_args: unknown) => {});
vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  return { ...original, answerWaslGate: (args: unknown) => answerWaslGate(args) };
});

beforeEach(() => answerWaslGate.mockClear());

const gate: WaslGateView = {
  host_id: "hostA",
  gate_id: "g1",
  kind: "consult",
  from: "alpha",
  summary: "alpha wants to consult beta",
  preview: "what is 2+2?",
  requested_at: "2026-07-03T00:00:00Z",
  deadline: "2026-07-03T00:02:00Z",
};

function renderShell(waslGates: WaslGateView[]) {
  const preloaded: Partial<VmState> = {
    snapshot: MOCK_SNAPSHOT,
    screen: "overview",
    waslGates,
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <AppShell>
          <div>main</div>
        </AppShell>
      </VmProvider>
    </ThemeProvider>
  );
}

describe("X5b — Wasl handoff-approval modal", () => {
  it("renders the pending gate's parties, summary, and preview", () => {
    renderShell([gate]);
    expect(screen.getByTestId("wasl-gate-modal")).toBeTruthy();
    expect(screen.getByTestId("wasl-gate-parties").textContent).toContain("alpha");
    expect(screen.getByTestId("wasl-gate-preview").textContent).toContain("2+2");
  });

  it("does NOT render when there are no pending gates (optionality)", () => {
    renderShell([]);
    expect(screen.queryByTestId("wasl-gate-modal")).toBeNull();
  });

  it("emits NO decision until the human clicks (never auto-approves)", () => {
    renderShell([gate]);
    expect(answerWaslGate).not.toHaveBeenCalled();
  });

  it("relays approve with the human's choice on click", async () => {
    renderShell([gate]);
    fireEvent.click(screen.getByTestId("wasl-gate-approve"));
    await waitFor(() =>
      expect(answerWaslGate).toHaveBeenCalledWith({
        host_id: "hostA",
        gate_id: "g1",
        outcome: "approved",
        reason: undefined,
      })
    );
  });

  it("relays deny on click", async () => {
    renderShell([gate]);
    fireEvent.click(screen.getByTestId("wasl-gate-deny"));
    await waitFor(() =>
      expect(answerWaslGate).toHaveBeenCalledWith(
        expect.objectContaining({ outcome: "denied", gate_id: "g1" })
      )
    );
  });

  it("shows a '+N more pending' hint with a queue", () => {
    renderShell([gate, { ...gate, gate_id: "g2" }]);
    expect(screen.getByTestId("wasl-gate-modal").textContent).toContain("more pending");
  });

  it("surfaces an error (not silently) when the relay fails, and re-enables", async () => {
    answerWaslGate.mockRejectedValueOnce(new Error("operator unreachable"));
    renderShell([gate]);
    fireEvent.click(screen.getByTestId("wasl-gate-approve"));
    await waitFor(() =>
      expect(screen.getByTestId("wasl-gate-error").textContent).toContain("operator unreachable")
    );
    // Button re-enabled for a retry (the human must not think they answered).
    expect((screen.getByTestId("wasl-gate-approve") as HTMLButtonElement).disabled).toBe(false);
  });
});
