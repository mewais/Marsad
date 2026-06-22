/**
 * GateOverlay + NewSession — behavioral Vitest/RTL tests.
 *
 * Pattern mirrors shell.test.tsx / Overview.test.tsx:
 *   inject MOCK_SNAPSHOT via VmProvider initialState, wrap in ThemeProvider,
 *   assert real behavior. API calls are mocked with vi.mock.
 */
import React from "react";
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { VmProvider, VmContext, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { GateOverlay } from "../src/screens/GateOverlay";
import { NewSession } from "../src/screens/NewSession";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot } from "../src/bridge/types";

// ---------------------------------------------------------------------------
// Mock the api module
// ---------------------------------------------------------------------------

vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  return {
    ...original,
    answerGate: vi.fn().mockResolvedValue(undefined),
    launchSession: vi.fn().mockResolvedValue("mock-session-new"),
  };
});

import * as api from "../src/bridge/api";

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function getAnswerGate() {
  return api.answerGate as ReturnType<typeof vi.fn>;
}
function getLaunchSession() {
  return api.launchSession as ReturnType<typeof vi.fn>;
}

/** Render GateOverlay with a seeded store pointing at sessionId. */
function renderGate(
  sessionId: string,
  snapshotOverride?: Partial<AppSnapshot>
) {
  const snapshot: AppSnapshot = { ...MOCK_SNAPSHOT, ...snapshotOverride };
  const preloaded: Partial<VmState> = {
    snapshot,
    screen: "gate",
    selectedId: sessionId,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <GateOverlay />
      </VmProvider>
    </ThemeProvider>
  );
}

/** Render NewSession modal (screen = "new-session"). */
function renderNewSession(snapshotOverride?: Partial<AppSnapshot>) {
  const snapshot: AppSnapshot = { ...MOCK_SNAPSHOT, ...snapshotOverride };
  const preloaded: Partial<VmState> = {
    snapshot,
    screen: "new-session",
    selectedId: null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <NewSession />
      </VmProvider>
    </ThemeProvider>
  );
}

// ---------------------------------------------------------------------------
// GateOverlay — Permission variant (s01)
// ---------------------------------------------------------------------------

describe("GateOverlay — permission variant", () => {
  it("renders the gate kind badge as 'Permission'", () => {
    renderGate("s01");
    expect(screen.getByTestId("gate-kind-badge").textContent).toBe("Permission");
  });

  it("renders the command from the gate payload", () => {
    renderGate("s01");
    expect(screen.getByTestId("gate-command").textContent).toContain(
      "prisma migrate deploy"
    );
  });

  it("renders the cwd from the gate payload", () => {
    renderGate("s01");
    expect(screen.getByTestId("gate-cwd").textContent).toContain("/app");
  });

  it("renders the risk warning block", () => {
    renderGate("s01");
    const risk = screen.getByTestId("gate-risk");
    expect(risk).toBeTruthy();
    expect(risk.textContent).toContain("medium");
  });

  it("renders the Approve button with label 'Approve & run'", () => {
    renderGate("s01");
    const btn = screen.getByTestId("approve-btn");
    expect(btn.textContent).toContain("Approve & run");
  });

  it("clicking Approve calls answerGate with allow:true and closes", async () => {
    renderGate("s01");
    fireEvent.click(screen.getByTestId("approve-btn"));
    // answerGate is mocked; assert it was called with allow=true
    await vi.waitFor(() => {
      expect(getAnswerGate()).toHaveBeenCalledWith(
        expect.objectContaining({ allow: true, session_id: "s01" })
      );
    });
  });

  it("clicking Reject… reveals the reason textarea", () => {
    renderGate("s01");
    // Reason field not visible initially
    expect(screen.queryByTestId("reject-reason")).toBeNull();
    fireEvent.click(screen.getByTestId("reject-btn"));
    expect(screen.getByTestId("reject-reason")).toBeTruthy();
  });

  it("after Reject… opens, Send rejection calls answerGate with allow:false", async () => {
    renderGate("s01");
    fireEvent.click(screen.getByTestId("reject-btn"));

    const textarea = screen.getByTestId("reject-reason");
    fireEvent.change(textarea, { target: { value: "Too risky" } });

    fireEvent.click(screen.getByTestId("send-rejection-btn"));

    await vi.waitFor(() => {
      expect(getAnswerGate()).toHaveBeenCalledWith(
        expect.objectContaining({
          allow: false,
          session_id: "s01",
          deny_reason: "Too risky",
        })
      );
    });
  });

  it("renders the session name in the header", () => {
    renderGate("s01");
    // Header shows session name
    const header = document.querySelector("[data-testid='gate-kind-badge']")!
      .closest("div")!.parentElement!;
    expect(header.textContent).toContain("api-auth-refactor");
  });
});

// ---------------------------------------------------------------------------
// GateOverlay — Plan variant (s02)
// ---------------------------------------------------------------------------

describe("GateOverlay — plan variant", () => {
  it("renders the gate kind badge as 'Plan'", () => {
    renderGate("s02");
    expect(screen.getByTestId("gate-kind-badge").textContent).toBe("Plan");
  });

  it("renders the plan steps list", () => {
    renderGate("s02");
    const steps = screen.getByTestId("gate-plan-steps");
    expect(steps).toBeTruthy();
    expect(steps.textContent).toContain("Extract queue interface");
    expect(steps.textContent).toContain("Implement AsyncWorker");
    expect(steps.textContent).toContain("Wire backpressure");
  });

  it("renders numbered steps (1, 2, 3)", () => {
    renderGate("s02");
    const steps = screen.getByTestId("gate-plan-steps");
    expect(steps.textContent).toContain("1");
    expect(steps.textContent).toContain("2");
    expect(steps.textContent).toContain("3");
  });

  it("Approve button label is 'Approve plan'", () => {
    renderGate("s02");
    expect(screen.getByTestId("approve-btn").textContent).toContain(
      "Approve plan"
    );
  });

  it("clicking Approve calls answerGate with allow:true for s02", async () => {
    renderGate("s02");
    fireEvent.click(screen.getByTestId("approve-btn"));
    await vi.waitFor(() => {
      expect(getAnswerGate()).toHaveBeenCalledWith(
        expect.objectContaining({ allow: true, session_id: "s02" })
      );
    });
  });
});

// ---------------------------------------------------------------------------
// GateOverlay — Edit variant (s03)
// ---------------------------------------------------------------------------

describe("GateOverlay — edit variant", () => {
  it("renders the gate kind badge as 'Edit'", () => {
    renderGate("s03");
    expect(screen.getByTestId("gate-kind-badge").textContent).toBe("Edit");
  });

  it("renders the diff view container", () => {
    renderGate("s03");
    expect(screen.getByTestId("gate-diff")).toBeTruthy();
  });

  it("diff view shows the filename", () => {
    renderGate("s03");
    // DiffView renders the filename in its header bar
    expect(screen.getByTestId("gate-diff").textContent).toContain(
      "src/features.py"
    );
  });

  it("Approve button label is 'Approve change'", () => {
    renderGate("s03");
    expect(screen.getByTestId("approve-btn").textContent).toContain(
      "Approve change"
    );
  });
});

// ---------------------------------------------------------------------------
// GateOverlay — no gate / closed state
// ---------------------------------------------------------------------------

describe("GateOverlay — closed state", () => {
  it("does not render modal content when screen is not 'gate'", () => {
    const preloaded: Partial<VmState> = {
      snapshot: MOCK_SNAPSHOT,
      screen: "overview",
      selectedId: "s01",
      tab: "Activity",
      filter: "All",
      search: "",
      wsMenuOpen: false,
    };
    render(
      <ThemeProvider initialTheme="dark">
        <VmProvider initialState={preloaded}>
          <GateOverlay />
        </VmProvider>
      </ThemeProvider>
    );
    // approve-btn only mounts when modal is open
    expect(screen.queryByTestId("approve-btn")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// NewSession — pickers render
// ---------------------------------------------------------------------------

describe("NewSession — pickers render", () => {
  it("renders project directory input", () => {
    renderNewSession();
    expect(screen.getByTestId("ns-project-input")).toBeTruthy();
  });

  it("renders recent dir chips", () => {
    renderNewSession();
    // At least one recent dir chip
    expect(screen.getByText("~/projects/api")).toBeTruthy();
  });

  it("clicking a recent dir chip fills the project input", () => {
    renderNewSession();
    fireEvent.click(screen.getByText("~/projects/api"));
    const input = screen.getByTestId("ns-project-input") as HTMLInputElement;
    expect(input.value).toBe("~/projects/api");
  });

  it("renders host pickers for all hosts in snapshot", () => {
    renderNewSession();
    // MOCK_SNAPSHOT has 3 hosts: local, hetzner, gpu
    expect(screen.getByTestId("ns-host-local")).toBeTruthy();
    expect(screen.getByTestId("ns-host-hetzner")).toBeTruthy();
    expect(screen.getByTestId("ns-host-gpu")).toBeTruthy();
  });

  it("renders model picker buttons", () => {
    renderNewSession();
    expect(screen.getByTestId("ns-model-claude-haiku-4-5")).toBeTruthy();
    expect(screen.getByTestId("ns-model-claude-sonnet-4-6")).toBeTruthy();
    expect(screen.getByTestId("ns-model-claude-opus-4-8")).toBeTruthy();
  });

  it("renders posture picker buttons", () => {
    renderNewSession();
    expect(screen.getByTestId("ns-posture-ask")).toBeTruthy();
    expect(screen.getByTestId("ns-posture-auto")).toBeTruthy();
    expect(screen.getByTestId("ns-posture-full")).toBeTruthy();
  });

  it("renders first prompt textarea", () => {
    renderNewSession();
    expect(screen.getByTestId("ns-first-prompt")).toBeTruthy();
  });

  it("renders Launch button", () => {
    renderNewSession();
    expect(screen.getByTestId("ns-launch-btn")).toBeTruthy();
  });

  it("renders 'Manage hosts →' link", () => {
    renderNewSession();
    expect(screen.getByTestId("ns-manage-hosts")).toBeTruthy();
    expect(screen.getByTestId("ns-manage-hosts").textContent).toContain(
      "Manage hosts"
    );
  });
});

// ---------------------------------------------------------------------------
// NewSession — Launch behavior
// ---------------------------------------------------------------------------

describe("NewSession — launch behavior", () => {
  it("Launch button is disabled when project is empty", () => {
    renderNewSession();
    const btn = screen.getByTestId("ns-launch-btn") as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
  });

  it("Launch button is enabled once project is filled", () => {
    renderNewSession();
    const input = screen.getByTestId("ns-project-input");
    fireEvent.change(input, { target: { value: "~/projects/new-app" } });
    const btn = screen.getByTestId("ns-launch-btn") as HTMLButtonElement;
    expect(btn.disabled).toBe(false);
  });

  it("clicking Launch calls launchSession with selected values", async () => {
    renderNewSession();

    // Fill project
    fireEvent.change(screen.getByTestId("ns-project-input"), {
      target: { value: "~/projects/test" },
    });

    // Pick Opus model
    fireEvent.click(screen.getByTestId("ns-model-claude-opus-4-8"));

    // Pick 'ask' posture
    fireEvent.click(screen.getByTestId("ns-posture-ask"));

    // Pick hetzner host
    fireEvent.click(screen.getByTestId("ns-host-hetzner"));

    // Fill first prompt
    fireEvent.change(screen.getByTestId("ns-first-prompt"), {
      target: { value: "Add unit tests" },
    });

    // Launch
    fireEvent.click(screen.getByTestId("ns-launch-btn"));

    await vi.waitFor(() => {
      expect(getLaunchSession()).toHaveBeenCalledWith(
        expect.objectContaining({
          project: "~/projects/test",
          host_id: "hetzner",
          model: "claude-opus-4-8",
          posture: "ask",
          first_prompt: "Add unit tests",
        })
      );
    });
  });

  it("Launch with no first prompt omits first_prompt from call", async () => {
    renderNewSession();

    fireEvent.change(screen.getByTestId("ns-project-input"), {
      target: { value: "~/projects/test" },
    });

    fireEvent.click(screen.getByTestId("ns-launch-btn"));

    await vi.waitFor(() => {
      const call = getLaunchSession().mock.calls[0][0];
      expect(call.first_prompt).toBeUndefined();
    });
  });

  it("'Manage hosts →' navigates to settings screen", () => {
    // Capture dispatch via VmContext consumer (imported at top of file)
    let capturedScreen: string | undefined;

    function StateCapture({ children }: { children: React.ReactNode }) {
      return (
        <VmContext.Consumer>
          {(ctx) => {
            if (ctx) capturedScreen = ctx.state.screen;
            return <>{children}</>;
          }}
        </VmContext.Consumer>
      );
    }

    const snapshot: AppSnapshot = { ...MOCK_SNAPSHOT };
    const preloaded: Partial<VmState> = {
      snapshot,
      screen: "new-session",
      selectedId: null,
      tab: "Activity",
      filter: "All",
      search: "",
      wsMenuOpen: false,
    };

    render(
      <ThemeProvider initialTheme="dark">
        <VmProvider initialState={preloaded}>
          <StateCapture>
            <NewSession />
          </StateCapture>
        </VmProvider>
      </ThemeProvider>
    );

    fireEvent.click(screen.getByTestId("ns-manage-hosts"));
    expect(capturedScreen).toBe("settings");
  });
});

// ---------------------------------------------------------------------------
// NewSession — closed state
// ---------------------------------------------------------------------------

describe("NewSession — closed state", () => {
  it("does not render modal content when screen is not 'new-session'", () => {
    const preloaded: Partial<VmState> = {
      snapshot: MOCK_SNAPSHOT,
      screen: "overview",
      selectedId: null,
      tab: "Activity",
      filter: "All",
      search: "",
      wsMenuOpen: false,
    };
    render(
      <ThemeProvider initialTheme="dark">
        <VmProvider initialState={preloaded}>
          <NewSession />
        </VmProvider>
      </ThemeProvider>
    );
    expect(screen.queryByTestId("ns-launch-btn")).toBeNull();
  });
});
