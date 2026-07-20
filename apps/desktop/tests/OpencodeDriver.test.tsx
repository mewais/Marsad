/**
 * OpencodeDriver — TDD red-phase tests for the opencode/ACP frontend contract.
 *
 * Spec: docs/specs/opencode-acp.md §4 (gates), §5 (features), §7 (frontend).
 *
 * Covers:
 *   1. "opencode" appears as a launchable driver in NodeModal (render + button).
 *   2. GateOverlay renders ACP permission_suggestions VERBATIM (name field, not
 *      the claude behavior/rules shape).  Clicking one calls answerGate with that
 *      option (so the backend can reply with optionId).
 *   3. Plan-mode: a session carrying plan entries renders them in the Activity tab.
 *
 * Pattern mirrors GateNewSession.test.tsx: VmProvider + ThemeProvider + fireEvent.
 * Tests 2 and 3 are expected to FAIL (red) until a coder extends the
 * implementation.  They MUST NOT fail with import/type errors.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { GateOverlay } from "../src/screens/GateOverlay";
import { NodeModal } from "../src/screens/NodeModal";
import { ActivityTab } from "../src/screens/tabs/Activity";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot, SessionView } from "../src/bridge/types";

// ---------------------------------------------------------------------------
// Mock the api module — mirrors GateNewSession.test.tsx exactly.
// We do NOT mock getSettings or refreshCapabilities here; the original api
// module handles them correctly in non-Tauri mode (getSettings returns
// DEFAULT_SETTINGS immediately; refreshCapabilities is a no-op).
// ---------------------------------------------------------------------------

vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  return {
    ...original,
    answerGate: vi.fn().mockResolvedValue(undefined),
    launchSession: vi.fn().mockResolvedValue("mock-session-opencode"),
    restartSession: vi.fn().mockResolvedValue(undefined),
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

/** Build a capabilities object for opencode on a given host. */
function opencodeCapabilities(hostId: string) {
  return {
    driver: "opencode",
    host_id: hostId,
    cli_available: true,
    launchable: true,
    models: {
      models: [{ id: "gpt-4o" }, { id: "claude-sonnet-4-5" }],
      source: "Listed" as const,
      free_text: true,
    },
    session_scope: null,
    sessions: [],
    fetched_at_ms: 0,
    errors: [],
  };
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

/** Render NodeModal (screen = "node-modal"). */
function renderNodeModal(snapshotOverride?: Partial<AppSnapshot>) {
  const snapshot: AppSnapshot = { ...MOCK_SNAPSHOT, ...snapshotOverride };
  const preloaded: Partial<VmState> = {
    snapshot,
    screen: "node-modal",
    selectedId: null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    capabilities: {
      "opencode::local": opencodeCapabilities("local"),
    },
    nodeModalCtx: { wsId: "ws-1", wsName: "Test WS", isFirstNode: true },
    draftWsId: null,
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <NodeModal />
      </VmProvider>
    </ThemeProvider>
  );
}

// ---------------------------------------------------------------------------
// §1 — "opencode" appears in the driver picker
// ---------------------------------------------------------------------------

describe("NodeModal — opencode driver", () => {
  it("renders a driver button for opencode (data-testid='nm-driver-opencode')", () => {
    // NodeModal renders one button per entry in DRIVERS with
    // data-testid="nm-driver-{id}".  DRIVERS includes "opencode", so this
    // should PASS immediately and confirms the driver is offered in the UI.
    renderNodeModal();
    const btn = screen.getByTestId("nm-driver-opencode");
    expect(btn).toBeTruthy();
  });

  it("the opencode driver button has non-empty label text", () => {
    renderNodeModal();
    const btn = screen.getByTestId("nm-driver-opencode");
    expect(btn.textContent?.trim().length).toBeGreaterThan(0);
  });

  it("the opencode driver button is enabled when capabilities mark it available", () => {
    renderNodeModal();
    const btn = screen.getByTestId("nm-driver-opencode") as HTMLButtonElement;
    // cli_available + launchable = true in the seeded capabilities.
    expect(btn.disabled).toBe(false);
  });

  it("clicking the opencode driver button does not throw and leaves it in the DOM", () => {
    renderNodeModal();
    const btn = screen.getByTestId("nm-driver-opencode");
    fireEvent.click(btn);
    expect(screen.getByTestId("nm-driver-opencode")).toBeTruthy();
  });

  it("the opencode driver button label contains 'opencode'", () => {
    renderNodeModal();
    const btn = screen.getByTestId("nm-driver-opencode");
    // The spec (§7) says label:"opencode" for this driver.
    expect(btn.textContent?.toLowerCase()).toContain("opencode");
  });
});

// ---------------------------------------------------------------------------
// §2 — GateOverlay renders ACP options VERBATIM (name/optionId/kind shape)
// ---------------------------------------------------------------------------
//
// ACP session/request_permission options carry { optionId, name, kind }.
// The existing suggestionLabel reads { behavior, rules } (claude's shape).
// These tests are RED until the coder extends suggestionLabel (or adds a
// separate rendering path) to use the ACP `name` field.

const ACP_OPTIONS = [
  { optionId: "a", name: "Allow once", kind: "allow_once" },
  { optionId: "b", name: "Always allow", kind: "allow_always" },
  { optionId: "c", name: "Reject", kind: "reject_once" },
];

/**
 * Inject ACP permission_suggestions onto s01's Permission gate.
 * Per spec §4: Gate::Permission reuses permission_suggestions to carry ACP
 * options verbatim.
 */
function withAcpSuggestions(suggestions: unknown[]): Partial<AppSnapshot> {
  const sessions = MOCK_SNAPSHOT.sessions.map((s) => {
    if (s.id !== "s01" || !s.gate || !("Permission" in s.gate)) return s;
    return {
      ...s,
      gate: {
        Permission: {
          ...(s.gate as { Permission: Record<string, unknown> }).Permission,
          permission_suggestions: suggestions,
        },
      },
    } as SessionView;
  });
  return { sessions };
}

describe("GateOverlay — ACP permission options (verbatim name) [RED]", () => {
  it("renders one suggestion button per ACP option with the ACP name verbatim", () => {
    // RED: suggestionLabel reads s.behavior (undefined for ACP options) and
    // s.rules (absent for ACP options), so the label becomes "allow" (the
    // defaulted behavior with no rules).  A coder must make suggestionLabel
    // (or an ACP-specific branch) use the `name` field instead.
    renderGate("s01", withAcpSuggestions(ACP_OPTIONS));

    expect(
      screen.getByTestId("approve-suggestion-0").textContent
    ).toContain("Allow once");
    expect(
      screen.getByTestId("approve-suggestion-1").textContent
    ).toContain("Always allow");
    expect(
      screen.getByTestId("approve-suggestion-2").textContent
    ).toContain("Reject");
  });

  it("clicking an ACP option calls answerGate with the optionId inside permission_updates", async () => {
    // The chosen optionId travels in permission_updates (the agent reads
    // permission_updates.optionId; the wire AnswerGate has no top-level optionId).
    renderGate("s01", withAcpSuggestions(ACP_OPTIONS));
    fireEvent.click(screen.getByTestId("approve-suggestion-0"));
    await vi.waitFor(() => {
      expect(getAnswerGate()).toHaveBeenCalledWith(
        expect.objectContaining({
          session_id: "s01",
          permission_updates: { optionId: ACP_OPTIONS[0].optionId },
        })
      );
    });
  });

  it("clicking the second ACP option calls answerGate with that option's optionId", async () => {
    renderGate("s01", withAcpSuggestions(ACP_OPTIONS));
    fireEvent.click(screen.getByTestId("approve-suggestion-1"));
    await vi.waitFor(() => {
      expect(getAnswerGate()).toHaveBeenCalledWith(
        expect.objectContaining({
          session_id: "s01",
          permission_updates: { optionId: ACP_OPTIONS[1].optionId },
        })
      );
    });
  });

  it("button label is the ACP name — NOT a fabricated 'allow <rule>' string", () => {
    // RED: with no `rules` array, suggestionLabel produces just "allow"
    // (the defaulted behavior).  For ACP options the button should read
    // "Allow once", "Always allow", "Reject" — the verbatim names.
    renderGate("s01", withAcpSuggestions([ACP_OPTIONS[0]]));
    const btn = screen.getByTestId("approve-suggestion-0");
    expect(btn.textContent).toContain("Allow once");
    // "allow" alone (the current fabricated output from the rules-empty path)
    // must NOT be the entire label text.
    expect(btn.textContent?.trim()).not.toBe("allow");
  });

  it("renders ACP option when it has no rules or behavior fields at all", () => {
    // ACP options never carry behavior/rules — only optionId/name/kind.
    const acpOnly = [{ optionId: "x", name: "Grant read access", kind: "allow_once" }];
    renderGate("s01", withAcpSuggestions(acpOnly));
    // RED: currently produces "allow" from the defaulted behavior/no-rules path.
    expect(screen.getByTestId("approve-suggestion-0").textContent).toContain(
      "Grant read access"
    );
  });

});

// ---------------------------------------------------------------------------
// §3 — Plan-mode: session carrying plan entries renders them
// ---------------------------------------------------------------------------
//
// The ACP `session/update{plan}` payload has:
//   { plan: { entries: [{ content: string, priority: "critical"|"high"|"medium"|"low" }] } }
//
// Per spec §5: "plan entries render in the transcript/activity (reuse plan
// rendering)".  The coder must add a `plan_entries` (or equivalent) field to
// SessionView and wire its rendering in ActivityTab (or a dedicated plan-mode
// panel component).
//
// ALL tests below are RED until that wiring exists.

function renderActivity(sessionId: string, snapshotOverride?: Partial<AppSnapshot>) {
  const snapshot: AppSnapshot = { ...MOCK_SNAPSHOT, ...snapshotOverride };
  const preloaded: Partial<VmState> = {
    snapshot,
    screen: "session",
    selectedId: sessionId,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <ActivityTab sessionId={sessionId} onOpenGate={() => undefined} />
      </VmProvider>
    </ThemeProvider>
  );
}

/**
 * Inject plan_entries onto session s04 (state:"Working", no gate — so the
 * Activity tab shows the transcript/plan area, not a gate modal).
 *
 * plan_entries does not exist on SessionView yet — the type cast is intentional:
 * we are writing the test BEFORE the coder adds the field.  The spread lets
 * the extra key pass through at runtime; the `as SessionView` silences the
 * type checker without touching source.
 */
function withPlanEntries(entries: { content: string; priority: string }[]): Partial<AppSnapshot> {
  const sessions = MOCK_SNAPSHOT.sessions.map((s) => {
    if (s.id !== "s04") return s;
    return { ...s, plan_entries: entries } as SessionView;
  });
  return { sessions };
}

describe("Activity tab — ACP plan entries (plan-mode) [RED]", () => {
  it("renders a plan-entries container when entries are non-empty", () => {
    // RED: SessionView does not yet carry plan_entries; ActivityTab does not
    // render them.  A coder must add the field + a data-testid="plan-entries"
    // container.
    const entries = [{ content: "First entry", priority: "high" }];
    renderActivity("s04", withPlanEntries(entries));
    expect(screen.getByTestId("plan-entries")).toBeTruthy();
  });

  it("renders all plan entry content strings", () => {
    const entries = [
      { content: "Design the API endpoints", priority: "critical" },
      { content: "Implement authentication layer", priority: "high" },
      { content: "Write integration tests", priority: "medium" },
    ];
    renderActivity("s04", withPlanEntries(entries));
    expect(screen.getByText(/Design the API endpoints/)).toBeTruthy();
    expect(screen.getByText(/Implement authentication layer/)).toBeTruthy();
    expect(screen.getByText(/Write integration tests/)).toBeTruthy();
  });

  it("renders plan entry priority labels alongside their content", () => {
    const entries = [
      { content: "Critical task first", priority: "critical" },
      { content: "Low priority cleanup", priority: "low" },
    ];
    renderActivity("s04", withPlanEntries(entries));
    expect(screen.getByText(/critical/i)).toBeTruthy();
    expect(screen.getByText(/low/i)).toBeTruthy();
  });

  it("renders one plan-entry element per entry (data-testid='plan-entry')", () => {
    const entries = [
      { content: "Entry one", priority: "high" },
      { content: "Entry two", priority: "medium" },
    ];
    renderActivity("s04", withPlanEntries(entries));
    const items = screen.getAllByTestId("plan-entry");
    expect(items).toHaveLength(2);
  });

});
