/**
 * U3c — compaction + rate-limit indicators in the Activity tab.
 *
 * U3b put compactions/last_compaction/rate_limit on SessionView; U3c populates
 * make_session_view (Rust, done) and surfaces them in the UI. These tests pin
 * the "nothing silently dropped" mandate: a compaction or a rate-limit event
 * must be visible; absent, no indicator renders.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { SessionDetail } from "../src/screens/SessionDetail";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot, SessionView } from "../src/bridge/types";

vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  return { ...original };
});

function renderSession(patch: Partial<SessionView>) {
  const base = MOCK_SNAPSHOT.sessions[0];
  const session: SessionView = { ...base, ...patch };
  const snapshot: AppSnapshot = {
    ...MOCK_SNAPSHOT,
    sessions: [session, ...MOCK_SNAPSHOT.sessions.slice(1)],
    selected_id: session.id,
  };
  const preloaded: Partial<VmState> = {
    snapshot,
    screen: "session",
    selectedId: session.id,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    capabilities: {},
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <SessionDetail />
      </VmProvider>
    </ThemeProvider>
  );
}

describe("U3c — compaction indicator", () => {
  it("renders with count + trigger when the session has compacted", () => {
    renderSession({
      compactions: 2,
      last_compaction: {
        trigger: "manual",
        pre_tokens: 100000,
        post_tokens: 20000,
        cumulative_dropped_tokens: 80000,
        duration_ms: 1200,
      },
    });
    const el = screen.getByTestId("compaction-indicator");
    expect(el.textContent).toContain("2");
    expect(el.textContent).toContain("manual");
    expect(el.textContent).toContain("80000");
  });

  it("does not render when the session has never compacted", () => {
    renderSession({ compactions: 0, last_compaction: null });
    expect(screen.queryByTestId("compaction-indicator")).toBeNull();
  });
});

describe("U3c — rate-limit indicator", () => {
  it("renders the status when the session has a rate_limit", () => {
    renderSession({
      rate_limit: {
        status: "allowed",
        resets_at: 0,
        rate_limit_type: "standard",
        overage_status: "none",
        is_using_overage: false,
      },
    });
    const el = screen.getByTestId("ratelimit-indicator");
    expect(el.textContent).toContain("allowed");
  });

  it("shows the overage note when using overage", () => {
    renderSession({
      rate_limit: {
        status: "limited",
        resets_at: 0,
        rate_limit_type: "standard",
        overage_status: "active",
        is_using_overage: true,
      },
    });
    expect(screen.getByTestId("ratelimit-indicator").textContent).toContain(
      "overage"
    );
  });

  it("does not render when the session has no rate_limit", () => {
    renderSession({ rate_limit: null });
    expect(screen.queryByTestId("ratelimit-indicator")).toBeNull();
  });
});
