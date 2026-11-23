import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import { ErrorBoundary, RenderErrorReport } from "../../src/ui";
import { quietBackend, renderApp, sessionByName } from "../helpers";

// The page a test makes fail. Everything else in the app is the real thing.
const crash = vi.hoisted(() => ({ page: false }));
vi.mock("../../src/features/config/ConfigPage", async (orig) => {
  const real = await orig<typeof import("../../src/features/config/ConfigPage")>();
  return { ...real, ConfigPage: (p: Parameters<typeof real.ConfigPage>[0]) => { if (crash.page) throw new Error("config page blew up"); return real.ConfigPage(p); } };
});

let logged: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  crash.page = false;
  // React and the boundary both log a render failure; the suite output stays readable.
  logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
});
afterEach(() => logged.mockRestore());

function Bomb({ explode }: { explode: boolean }): JSX.Element {
  if (explode) throw new Error("kaboom");
  return <span>fine</span>;
}

describe("ErrorBoundary", () => {
  it("shows what failed instead of unmounting everything, and draws again on request", async () => {
    const user = userEvent.setup();
    function Host() {
      const [explode, setExplode] = useState(true);
      return (
        <div>
          <button type="button" onClick={() => setExplode(false)}>repair</button>
          <ErrorBoundary variant="page" name="test.page" what="this page"><Bomb explode={explode} /></ErrorBoundary>
          <p>the rest of the window</p>
        </div>
      );
    }
    render(<Host />);
    const fallback = screen.getByTestId("boundary-page");
    expect(fallback).toHaveTextContent("Could not display this page");
    expect(fallback).toHaveTextContent("kaboom");
    expect(screen.getByText("the rest of the window")).toBeInTheDocument();
    expect(logged).toHaveBeenCalled();
    // Reloading it while it still fails shows the failure again; once fixed it draws.
    await user.click(screen.getByRole("button", { name: "Reload this page" }));
    expect(screen.getByTestId("boundary-page")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "repair" }));
    await user.click(screen.getByRole("button", { name: "Reload this page" }));
    expect(screen.getByText("fine")).toBeInTheDocument();
    expect(screen.queryByTestId("boundary-page")).toBeNull();
  });

  it("draws again by itself when its resetKey changes (another page, a newer item)", () => {
    const { rerender } = render(<ErrorBoundary variant="item" name="test.item" what="this entry" resetKey="a"><Bomb explode /></ErrorBoundary>);
    expect(screen.getByTestId("boundary-item")).toHaveTextContent("Could not display this entry: kaboom");
    rerender(<ErrorBoundary variant="item" name="test.item" what="this entry" resetKey="b"><Bomb explode={false} /></ErrorBoundary>);
    expect(screen.getByText("fine")).toBeInTheDocument();
  });

  it("the app-level variant offers to reload the window", async () => {
    const reload = vi.fn();
    Object.defineProperty(window, "location", { configurable: true, value: { ...window.location, reload } });
    const user = userEvent.setup();
    render(<ErrorBoundary variant="app" name="test.app" what="the window"><Bomb explode /></ErrorBoundary>);
    expect(screen.getByTestId("boundary-app")).toHaveTextContent("cannot show this window");
    await user.click(screen.getByRole("button", { name: "Reload the window" }));
    expect(reload).toHaveBeenCalled();
  });
});

describe("reporting", () => {
  it("a failure is reported once, by the boundary's name and nothing else", async () => {
    const user = userEvent.setup();
    const report = vi.fn();
    render(<RenderErrorReport.Provider value={report}><ErrorBoundary variant="page" name="test.page" what="this page"><Bomb explode /></ErrorBoundary></RenderErrorReport.Provider>);
    expect(report.mock.calls).toEqual([["test.page"]]);
    // Reloading while it still fails is another failure, reported as one.
    await user.click(screen.getByRole("button", { name: "Reload this page" }));
    expect(report).toHaveBeenCalledTimes(2);
    expect(JSON.stringify(report.mock.calls)).not.toContain("kaboom");
  });

  it("a boundary outside the app reports nowhere and still shows the failure", () => {
    render(<ErrorBoundary variant="item" name="test.item" what="this entry"><Bomb explode /></ErrorBoundary>);
    expect(screen.getByTestId("boundary-item")).toBeInTheDocument();
  });
});

describe("in the app", () => {
  it("a page that throws is replaced by the fallback; the shell and the other pages keep working", async () => {
    const user = userEvent.setup();
    const r = await renderApp("/inbox");
    crash.page = true;
    act(() => r.history.push("/config"));
    const fallback = await screen.findByTestId("boundary-page");
    expect(fallback).toHaveTextContent("Could not display this page");
    expect(fallback).toHaveTextContent("config page blew up");
    // The app's log has the failure: the boundary's name, never the message.
    await waitFor(() => expect(r.backend.diagEvents.filter((e) => e.ev === "ui.render_error")).toEqual([{ ev: "ui.render_error", component: "page.config" }]));
    expect(JSON.stringify(r.backend.diagEvents)).not.toContain("blew up");
    // The title bar, rail and status bar are still there and work.
    const rail = screen.getByRole("navigation", { name: "Main" });
    expect(screen.getByRole("contentinfo", { name: "Status" })).toBeInTheDocument();
    await user.click(within(rail).getByRole("link", { name: /Inbox/ }));
    await waitFor(() => expect(document.querySelectorAll("[data-session]").length).toBeGreaterThan(0));
    expect(screen.queryByTestId("boundary-page")).toBeNull();
    // Reloading the failed page after it is fixed brings it back.
    crash.page = false;
    act(() => r.history.push("/config"));
    await waitFor(() => expect(screen.queryByTestId("boundary-page")).toBeNull());
    expect(screen.getByRole("heading", { name: "Agent config" })).toBeInTheDocument();
  });

  it("the failed page offers a way out and a reload of just that page", async () => {
    const user = userEvent.setup();
    const r = await renderApp("/inbox");
    crash.page = true;
    act(() => r.history.push("/config"));
    await screen.findByTestId("boundary-page");
    crash.page = false;
    await user.click(screen.getByRole("button", { name: "Reload this page" }));
    expect(screen.queryByTestId("boundary-page")).toBeNull();
    crash.page = true;
    act(() => r.history.push("/hosts"));
    act(() => r.history.push("/config"));
    await user.click(await screen.findByRole("button", { name: "Back to the Inbox" }));
    expect(r.history.store.get()).toBe("/inbox");
  });

  it("one transcript entry that cannot be drawn does not take the transcript down", async () => {
    // A tool call whose arguments cannot be turned into a summary: drawing that one row throws.
    const backend: FixtureBackend = quietBackend();
    const s = sessionByName(backend, "docs-search-index");
    const poison = { toJSON() { throw new Error("cannot summarise"); } };
    await backend.request("session.command", { id: s.id, command: { command: "prompt", text: "before the bad one" } }, { host: s.host });
    const entry = (backend as unknown as { hosts: Map<string, { sessions: Map<string, { events: unknown[] }> }> }).hosts.get(s.host)!.sessions.get(s.id)!;
    const last = (entry.events[entry.events.length - 1] as { cursor: number }).cursor;
    entry.events.push({ cursor: last + 1, t: Date.now(), kind: "agent", event: { type: "tool_call_started", name: "Weird", call_id: "bad", input: poison, is_subagent: false } });
    entry.events.push({ cursor: last + 2, t: Date.now(), kind: "agent", event: { type: "assistant_delta", text: "after the bad one", message_id: "m-after" } });
    await renderApp(`/session/${s.host}/${s.id}`, backend);
    const bad = await screen.findByTestId("boundary-item");
    expect(bad).toHaveTextContent("Could not display this entry: cannot summarise");
    expect(screen.getByText("before the bad one")).toBeInTheDocument();
    expect(screen.getByText("after the bad one")).toBeInTheDocument();
    expect(screen.queryByTestId("boundary-page")).toBeNull();
    await waitFor(() => expect(backend.diagEvents.filter((e) => e.ev === "ui.render_error").map((e) => (e as { component: string }).component)).toEqual(["transcript_entry"]));
    // The composer next to the transcript still works.
    expect(screen.getByRole("textbox", { name: "Message" })).toBeEnabled();
  });
});
