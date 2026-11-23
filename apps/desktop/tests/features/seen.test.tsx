import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { hold, mountApp, quietBackend, renderApp, sessionByName } from "../helpers";

const rowOf = (key: string) => document.querySelector(`[data-session="${key}"]`) as HTMLElement;

describe("'finished since you looked at'", () => {
  it("names a time only when a seen marker exists; otherwise it says nothing about one (no epoch time)", async () => {
    const backend = quietBackend();
    backend.hub.seen = [];
    await renderApp("/inbox", backend);
    const line = screen.getByText(/finished/, { selector: ".c-done" });
    expect(line).toHaveTextContent(/\d+ finished$/);
    expect(line.textContent).not.toMatch(/since you looked at|\d\d:\d\d/);
  });

  it("says the time once the human has marked something seen, and not while the markers are still loading", async () => {
    const backend = quietBackend();
    const h = hold(backend, "store.seen.list");
    mountApp("/inbox", backend);
    await waitFor(() => expect(h.pending()).toBe(1));
    await screen.findAllByText(/finished/, { selector: ".c-done" });
    expect(screen.getByText(/finished/, { selector: ".c-done" }).textContent).not.toMatch(/since you looked at/);
    act(() => h.release());
    await waitFor(() => expect(screen.getByText(/finished/, { selector: ".c-done" })).toHaveTextContent(/since you looked at \d{1,2}:\d\d/));
  });
});

describe("seen markers are what the daemon saved", () => {
  it("a session is not shown as seen until the daemon confirms it", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    await renderApp("/inbox", backend);
    const key = sessionByName(backend, "ci-repro-windows").key;
    expect(rowOf(key)).toHaveAttribute("data-status", "done");
    const h = hold(backend, "store.seen.mark");
    await user.click(within(rowOf(key)).getByRole("button", { name: /Mark seen/ }));
    await waitFor(() => expect(h.pending()).toBe(1));
    expect(rowOf(key)).toHaveAttribute("data-status", "done");
    act(() => h.release());
    await waitFor(() => expect(rowOf(key)).toHaveAttribute("data-status", "idle"));
    expect(backend.hub.seen.some((m) => `${m.host}/${m.session}` === key)).toBe(true);
  });

  it("a refused marker leaves it unseen and says so", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    await renderApp("/inbox", backend);
    const key = sessionByName(backend, "ci-repro-windows").key;
    const h = hold(backend, "store.seen.mark");
    await user.click(within(rowOf(key)).getByRole("button", { name: /Mark seen/ }));
    await waitFor(() => expect(h.pending()).toBe(1));
    act(() => h.reject({ code: "internal", message: "store is read-only" }));
    expect(await screen.findByText("store is read-only")).toBeInTheDocument();
    expect(rowOf(key)).toHaveAttribute("data-status", "done");
  });

  it("'Mark all seen' takes its result from the daemon's reply, with a pending state meanwhile", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    await renderApp("/inbox", backend);
    const key = sessionByName(backend, "ci-repro-windows").key;
    const h = hold(backend, "store.seen.mark_all");
    const [header] = screen.getAllByRole("button", { name: "Mark all seen" });
    await user.click(header);
    await waitFor(() => expect(h.pending()).toBe(1));
    expect(header).toHaveAttribute("aria-busy", "true");
    expect(rowOf(key)).toHaveAttribute("data-status", "done");
    act(() => h.release());
    await waitFor(() => expect(rowOf(key)).toHaveAttribute("data-status", "idle"));
  });

  it("markers that cannot be loaded are said so (every finished session would otherwise look unseen), with a way to try again", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const h = hold(backend, "store.seen.list");
    h.failAll({ code: "internal", message: "seen.json is unreadable" });
    mountApp("/inbox", backend);
    await waitFor(() => expect(screen.getByTestId("connection-banner")).toHaveTextContent(/Could not load which sessions you have seen/));
    expect(screen.getByTestId("connection-banner")).toHaveTextContent("seen.json is unreadable");
    h.healAll();
    await user.click(within(screen.getByTestId("connection-banner")).getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(h.pending()).toBe(1));
    act(() => h.release());
    await waitFor(() => expect(screen.queryByTestId("connection-banner")).toBeNull());
  });
});
