import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { DriverInventory, SessionSnapshot } from "../../src/bridge/wire";
import { driverVersionState, sessionModesState, sessionModelsState, sessionVersionState } from "../../src/state";
import { hold, renderApp, sessionByName, quietBackend } from "../helpers";

async function open(name: string, backend = quietBackend()) {
  const s = sessionByName(backend, name);
  const r = await renderApp(`/session/${s.host}/${s.id}`, backend);
  await screen.findByRole("heading", { name });
  return { ...r, s };
}

describe("Session view", () => {
  it("renders markdown, collapsed thinking, tool calls, subagent and inline gates", async () => {
    const user = userEvent.setup();
    await open("migrate-auth-middleware");
    expect(await screen.findByText(/find every place that reads/)).toBeInTheDocument();
    expect(document.querySelector(".msg-ai code")?.textContent).toBe("req.session"); // inline `code` rendered
    const think = screen.getByRole("button", { name: /Thinking/ });
    expect(think).toHaveAttribute("aria-expanded", "false");
    await user.click(think);
    expect(screen.getByText(/I need to find every place/)).toBeInTheDocument();
    expect(screen.getByTestId("subagent")).toHaveTextContent("Subagent · Explore");
    const grep = screen.getByRole("button", { name: /Grep/ });
    await user.click(grep);
    expect(document.querySelector(".tool-b")?.textContent).toContain("routes/login.ts:22");
    // The Edit and the failing Bash that follow the subagent are one run of tool calls: open it first.
    const run = screen.getByRole("button", { name: /2 tool calls.*1 failed/ });
    expect(run).toHaveAttribute("aria-expanded", "false");
    await user.click(run);
    await user.click(screen.getByRole("button", { name: /Edit apps\/api\/src\/server.ts/ }));
    expect(screen.getByTestId("tool-diff")).toBeInTheDocument();
    expect(within(document.querySelector(".run-b") as HTMLElement).getByRole("button", { name: /Bash.*failed/ })).toBeInTheDocument();
    expect(screen.getByText(/Question from Claude Code/)).toBeInTheDocument();
    expect(screen.getByText(/Permission needed · Bash/)).toBeInTheDocument();
    expect(screen.getByText("Waiting for you · 1 permission, 1 question")).toBeInTheDocument();
    expect(screen.getByText(/Plan · 3 of 6/i)).toBeInTheDocument();
  });

  it("terminal tab is honest when the host cannot give a terminal", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    hold(backend, "pty.list").failAll({ code: "unsupported", message: "Terminals need the Marsad helper on a real host." });
    await open("docs-search-index", backend);
    await user.click(screen.getByRole("tab", { name: "Terminal" }));
    expect(await screen.findByText("Terminal not available")).toBeInTheDocument();
    expect(screen.getByText(/need the Marsad helper/)).toBeInTheDocument();
  });

  it("composer: slash menu comes from the snapshot, mode and model pickers too, and sending fires a prompt", async () => {
    const user = userEvent.setup();
    const { backend, s } = await open("docs-search-index");
    const box = screen.getByRole("textbox", { name: "Message" });
    await user.type(box, "/");
    const list = screen.getByRole("listbox", { name: "Slash commands" });
    expect(within(list).getAllByRole("option").map((o) => o.textContent)).toEqual(expect.arrayContaining([expect.stringContaining("/init")]));
    await user.clear(box);
    await user.click(screen.getByRole("button", { name: /Mode: Build/ }));
    expect(screen.getByRole("option", { name: /Plan/ })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await user.type(box, "add a sitemap{Enter}");
    await waitFor(() => expect(backend.requests.some((r) => (r.params as { command?: { text?: string } })?.command?.text === "add a sitemap")).toBe(true));
    expect(await screen.findByText("add a sitemap")).toBeInTheDocument();
    expect(box).toHaveValue("");
    // the scripted agent replies and the session finishes
    expect(await screen.findByText(/Nothing else needs your attention/, undefined, { timeout: 3000 })).toBeInTheDocument();
    void s;
  });

  it("shows loading… while live session models, modes, and version arrive asynchronously, and Not reported when unsupported or ended", async () => {
    const snap = (over: Partial<SessionSnapshot> = {}): SessionSnapshot =>
      ({
        phase: { state: "starting" },
        capabilities: { set_model: true, set_mode: true } as SessionSnapshot["capabilities"],
        available_models: [],
        available_modes: [],
        cli_version: null,
        native_session_id: null,
        model: null,
        turns_completed: 0,
        turn: null,
        ...over,
      }) as SessionSnapshot;

    // Live + capable + empty -> loading
    expect(sessionModelsState(snap())).toBe("loading");
    expect(sessionModesState(snap())).toBe("loading");
    expect(sessionVersionState(snap())).toBe("loading");

    // Stale (disconnected host) -> not_reported (nothing arriving over SSH right now)
    expect(sessionModelsState(snap(), true)).toBe("not_reported");
    expect(sessionModesState(snap(), true)).toBe("not_reported");
    expect(sessionVersionState(snap(), true)).toBe("not_reported");

    // Capability false -> not_reported even while live
    expect(sessionModelsState(snap({ capabilities: { set_model: false, set_mode: false } as SessionSnapshot["capabilities"] }))).toBe("not_reported");
    expect(sessionModesState(snap({ capabilities: { set_model: false, set_mode: false } as SessionSnapshot["capabilities"] }))).toBe("not_reported");

    // Version after init arrived (idle + model/native_session_id reported without cli_version) -> not_reported
    expect(sessionVersionState(snap({ phase: { state: "idle" }, model: "m1" }))).toBe("not_reported");
    // Session ended -> not_reported
    expect(sessionModelsState(snap({ phase: { state: "exited", code: 0 } }))).toBe("not_reported");

    // Driver inventory version state
    const drv = (over: Partial<DriverInventory> = {}): DriverInventory =>
      ({ installed: false, version: null, refreshing: true, refreshed_at_ms: 0, probes: [], ...over }) as DriverInventory;
    expect(driverVersionState(drv(), true)).toEqual({ kind: "loading" });
    expect(driverVersionState(drv({ installed: true }), true)).toEqual({ kind: "loading" });
    expect(driverVersionState(drv({ installed: true, refreshing: false, probes: [{ kind: "version", ok: true, duration_ms: 10 }] }), true)).toEqual({ kind: "not_reported" });
    expect(driverVersionState(drv({ installed: true, version: "2.1.4" }), true)).toEqual({ kind: "ready", version: "2.1.4" });
    expect(driverVersionState(drv({ refreshing: false, refreshed_at_ms: 100 }), true)).toEqual({ kind: "not_installed" });

    // Rendered UI: live session with empty lists + capable -> Composer and ConfigTab show loading… instead of hiding/Not reported
    const backend = quietBackend();
    const s = sessionByName(backend, "docs-search-index");
    backend.patchSession(s.host, s.id, { phase: { state: "starting" }, available_models: [], available_modes: [], cli_version: null, model: null, mode: null, native_session_id: null, turns_completed: 0 });
    const user = userEvent.setup();
    await open("docs-search-index", backend);
    expect(screen.getByTestId("composer-modes-loading")).toHaveTextContent("loading…");
    expect(screen.getByTestId("composer-models-loading")).toHaveTextContent("loading…");
    expect(screen.getByTestId("session-version-loading")).toHaveTextContent("loading…");

    await user.click(screen.getByRole("tab", { name: "Config" }));
    expect(screen.getByRole("button", { name: "Model: Loading…" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Mode: Loading…" })).toBeInTheDocument();

    // Once capabilities say not supported, ConfigTab falls back to Not reported and Composer hides them
    act(() => {
      backend.patchSession(s.host, s.id, { capabilities: { ...s.snap.capabilities, set_model: false, set_mode: false } });
    });
    expect(await screen.findByRole("button", { name: "Model: Not reported" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Mode: Not reported" })).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Activity" }));
    expect(screen.queryByTestId("composer-modes-loading")).toBeNull();
    expect(screen.queryByTestId("composer-models-loading")).toBeNull();
  });
});
