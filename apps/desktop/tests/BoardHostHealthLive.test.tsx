/**
 * BUG #1 — the Board host-health strip must live-update on `host_health` /
 * `hosts` vm-updates, without a navigate-away-and-back refetch.
 *
 * Before the fix, BoardScreen passed the statically-fetched `board.host_chips`
 * (fetched ONCE via fetchBoard()) straight to <Board>, so live host status
 * arriving into `state.snapshot.hosts` never reached the chips. The fix derives
 * the strip's chips from the live `state.snapshot.hosts` source.
 *
 * Harness mirrors useVmWorkstreams.test.tsx: VmProvider + a Probe that captures
 * the live ctx so the test can dispatch a VM_UPDATE, plus BoardScreen rendered
 * for the actual DOM assertions.
 */
import { describe, it, expect } from "vitest";
import { render, screen, act, waitFor } from "@testing-library/react";
import React from "react";
import { VmProvider, VmContext, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { BoardScreen } from "../src/screens/BoardScreen";
import { MOCK_BOARD } from "../src/bridge/mockBoard";
import type { AppSnapshot, Host } from "../src/bridge/types";

function makeHost(overrides: Partial<Host> & { id: string }): Host {
  return {
    label: overrides.id,
    target: "dev@example.com",
    status: "Connected",
    session_count: 0,
    is_local: false,
    removable: true,
    ...overrides,
  };
}

function StateCapture({
  onCtx,
  children,
}: {
  onCtx: (ctx: NonNullable<React.ContextType<typeof VmContext>>) => void;
  children: React.ReactNode;
}) {
  return (
    <VmContext.Consumer>
      {(ctx) => {
        if (ctx) onCtx(ctx);
        return <>{children}</>;
      }}
    </VmContext.Consumer>
  );
}

function renderBoardScreen(hosts: Host[]) {
  let dispatch: React.Dispatch<any> = () => {};
  const snapshot = {
    // Minimal snapshot shape — BoardScreen only reads snapshot.hosts.
    sessions: [],
    hosts,
    workspaces: [],
    active_ws: "",
    selected_id: null,
  } as unknown as AppSnapshot;
  const preloaded: Partial<VmState> = {
    snapshot,
    screen: "board",
    board: MOCK_BOARD,
  };
  render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <StateCapture onCtx={(ctx) => (dispatch = ctx.dispatch)}>
          <BoardScreen />
        </StateCapture>
      </VmProvider>
    </ThemeProvider>
  );
  return { getDispatch: () => dispatch };
}

describe("Board host-health strip — live updates (BUG #1)", () => {
  it("derives host chips from the live snapshot.hosts (not the static board.host_chips)", () => {
    renderBoardScreen([
      makeHost({ id: "gpu-worker", label: "gpu-worker", status: "Connected" }),
    ]);
    const chip = screen.getByTestId("host-chip-gpu-worker");
    expect(chip).toBeTruthy();
    expect(chip.getAttribute("data-status")).toBe("connected");
    // The static MOCK_BOARD chips (e.g. build-host) are NOT the live source.
    expect(screen.queryByTestId("host-chip-build-host")).toBeNull();
  });

  it("reflects a host_health update WITHOUT a refetch/navigation", async () => {
    const { getDispatch } = renderBoardScreen([
      makeHost({ id: "gpu-worker", label: "gpu-worker", status: "Connected" }),
    ]);
    expect(screen.getByTestId("host-chip-gpu-worker").getAttribute("data-status")).toBe(
      "connected"
    );

    act(() => {
      getDispatch()({
        type: "VM_UPDATE",
        payload: { kind: "host_health", host_id: "gpu-worker", status: "Unreachable" },
      });
    });

    await waitFor(() => {
      expect(
        screen.getByTestId("host-chip-gpu-worker").getAttribute("data-status")
      ).toBe("unreachable");
    });
  });

  it("reflects a full 'hosts' replacement update live", async () => {
    const { getDispatch } = renderBoardScreen([
      makeHost({ id: "h1", label: "h1", status: "Connected" }),
    ]);
    expect(screen.getByTestId("host-chip-h1")).toBeTruthy();

    act(() => {
      getDispatch()({
        type: "VM_UPDATE",
        payload: {
          kind: "hosts",
          hosts: [makeHost({ id: "h2", label: "h2", status: "Reconnecting" })],
        },
      });
    });

    await waitFor(() => {
      const chip = screen.getByTestId("host-chip-h2");
      expect(chip).toBeTruthy();
      expect(chip.getAttribute("data-status")).toBe("reconnecting");
      expect(screen.queryByTestId("host-chip-h1")).toBeNull();
    });
  });
});
