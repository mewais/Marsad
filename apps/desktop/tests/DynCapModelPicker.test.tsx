/**
 * U2 — NewSession model picker consumes ONLY dynamic capabilities.
 *
 * Contract pinned (docs/specs/dynamic-capabilities.md §5, "NewSession model
 * picker"):
 *   "options = capabilities[(driver, host)].models from the useVm store for
 *   the selected host + driver — never an import. source == Observed →
 *   render an 'unverified — models seen in past sessions' badge and always
 *   show the free-text model input; empty catalog → free-text input only.
 *   Default selection = persisted last-used model from settings if present,
 *   else the first catalog entry, else empty — no `DEFAULT_MODEL` constant
 *   anywhere."
 *
 * Today `NewSession.tsx` imports `MODELS`/`DEFAULT_MODEL` from
 * `bridge/models.ts` (the kill-list file, §5) and renders that static list
 * regardless of any capability state — this suite is RED until U2 rewires
 * the picker onto a `capabilities` VM slice. See DynCapKillList.test.tsx for
 * the corresponding grep gate.
 *
 * Harness pattern copied from tests/GateNewSession.test.tsx (VmProvider
 * `initialState` injection + `vi.mock` of `bridge/api`). The `capabilities`
 * slice does not exist on `VmState` yet (§3.6); it is injected via a cast so
 * this file runs today (and will type-check for real once §3.6 lands) —
 * `pnpm test` here is Vitest only (esbuild transform, no tsc gate).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { NewSession } from "../src/screens/NewSession";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot } from "../src/bridge/types";
import { capsMap, makeCapabilities, type CapabilitiesViewFixture } from "./fixtures/capabilities";

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

// The three preset ids the kill-list requires gone from every UI surface
// (dynamic-capabilities.md §5 grep gate #2: `claude-(opus|sonnet|haiku)-[0-9]`).
const PRESET_MODEL_IDS = ["claude-opus-4-8", "claude-sonnet-4-6", "claude-haiku-4-5"];

function renderNewSession(caps: CapabilitiesViewFixture[]) {
  const snapshot: AppSnapshot = { ...MOCK_SNAPSHOT };
  const preloaded = {
    snapshot,
    screen: "new-session",
    selectedId: null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    // Not a real VmState field until §3.6 lands — injected for forward-compat.
    capabilities: capsMap(caps),
  } as unknown as Partial<VmState>;
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <NewSession />
      </VmProvider>
    </ThemeProvider>
  );
}

function presetButtonsPresent(): boolean {
  return PRESET_MODEL_IDS.some((id) => screen.queryByTestId(`ns-model-${id}`) != null);
}

describe("NewSession model picker — capabilities-only source (U2 contract)", () => {
  it("renders model options ONLY from capabilities[('claude','local')].models — never the static preset ids", () => {
    renderNewSession([
      makeCapabilities({
        driver: "claude",
        host_id: "local",
        models: {
          models: [{ id: "claude-opus-4-9-preview" }, { id: "claude-4-custom-eval" }],
          source: "Observed",
          free_text: true,
        },
        fetched_at_ms: 1783100000000,
      }),
    ]);

    expect(screen.getByTestId("ns-model-claude-opus-4-9-preview")).toBeTruthy();
    expect(screen.getByTestId("ns-model-claude-4-custom-eval")).toBeTruthy();
    expect(presetButtonsPresent()).toBe(false);
  });

  it("switching the selected host swaps the rendered model set to that host's capabilities", () => {
    renderNewSession([
      makeCapabilities({
        driver: "claude",
        host_id: "local",
        models: { models: [{ id: "local-only-model" }], source: "Observed", free_text: true },
      }),
      makeCapabilities({
        driver: "claude",
        host_id: "hetzner",
        models: { models: [{ id: "hetzner-only-model" }], source: "Observed", free_text: true },
      }),
    ]);

    expect(screen.getByTestId("ns-model-local-only-model")).toBeTruthy();
    expect(screen.queryByTestId("ns-model-hetzner-only-model")).toBeNull();

    fireEvent.click(screen.getByTestId("ns-host-hetzner"));

    expect(screen.getByTestId("ns-model-hetzner-only-model")).toBeTruthy();
    expect(screen.queryByTestId("ns-model-local-only-model")).toBeNull();
  });

  it("source: Observed renders the 'unverified' badge and always shows a free-text model field", () => {
    renderNewSession([
      makeCapabilities({
        driver: "claude",
        host_id: "local",
        models: { models: [{ id: "observed-model-a" }], source: "Observed", free_text: true },
      }),
    ]);

    const badge = screen.getByTestId("ns-model-unverified-badge");
    expect(badge.textContent).toMatch(/unverified/i);
    expect(screen.getByTestId("ns-model-freetext")).toBeTruthy();
  });

  it("source: Listed does NOT render the unverified badge (still may show free text per free_text flag)", () => {
    renderNewSession([
      makeCapabilities({
        driver: "claude",
        host_id: "local",
        models: { models: [{ id: "listed-model-a" }], source: "Listed", free_text: false },
      }),
    ]);

    expect(screen.queryByTestId("ns-model-unverified-badge")).toBeNull();
    // agy-shaped catalogs (Listed, free_text:false) must NOT show free text —
    // "an unknown model is a designated error case" (proto §1.1).
    expect(screen.queryByTestId("ns-model-freetext")).toBeNull();
  });

  it("empty catalog renders free-text entry only — an honest hint, never a fallback preset list", () => {
    renderNewSession([
      makeCapabilities({ driver: "claude", host_id: "local" }), // default = empty catalog
    ]);

    expect(presetButtonsPresent()).toBe(false);
    expect(screen.getByTestId("ns-model-freetext")).toBeTruthy();
    expect(screen.getByTestId("ns-model-empty-hint")).toBeTruthy();

    // No leftover model-option buttons beyond the free-text/badge/hint surface.
    const modelTestIds = Array.from(
      document.querySelectorAll('[data-testid^="ns-model-"]')
    ).map((el) => el.getAttribute("data-testid"));
    const optionButtons = modelTestIds.filter(
      (id) => id !== "ns-model-freetext" && id !== "ns-model-empty-hint" && id !== "ns-model-unverified-badge"
    );
    expect(optionButtons).toEqual([]);
  });

  it("default selection falls back to the first catalog entry when the persisted default_model isn't in the catalog — no DEFAULT_MODEL constant", async () => {
    // Mock-mode getSettings() resolves DEFAULT_SETTINGS.default_model ===
    // "claude-sonnet-4-6" (bridge/api.ts), which is NOT in this fixture's
    // catalog — so the contract (§5) requires falling back to the first
    // catalog entry, not any hardcoded id.
    renderNewSession([
      makeCapabilities({
        driver: "claude",
        host_id: "local",
        models: {
          models: [{ id: "catalog-first" }, { id: "catalog-second" }],
          source: "Observed",
          free_text: true,
        },
      }),
    ]);

    fireEvent.change(screen.getByTestId("ns-project-input"), {
      target: { value: "~/projects/probe" },
    });
    fireEvent.click(screen.getByTestId("ns-launch-btn"));

    await vi.waitFor(() => {
      expect(api.launchSession as ReturnType<typeof vi.fn>).toHaveBeenCalledWith(
        expect.objectContaining({ model: "catalog-first" })
      );
    });
  });
});
