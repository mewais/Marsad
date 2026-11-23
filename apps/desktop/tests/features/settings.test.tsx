import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import pkg from "../../package.json";
import { hold, mountApp, quietBackend, renderApp } from "../helpers";

async function settings(s: string) {
  const backend = quietBackend();
  const r = await renderApp("/inbox", backend);
  act(() => r.history.push(`/settings?s=${s}`));
  return r;
}

describe("settings", () => {
  it("general: theme applies at once and is saved in the hub's settings", async () => {
    const user = userEvent.setup();
    const { backend } = await settings("general");
    await user.click(await screen.findByRole("button", { name: "Light" }));
    expect(document.documentElement.getAttribute("data-theme")).toBe("light");
    await waitFor(() => expect(backend.requests.find((r) => r.method === "store.settings.set")?.params).toEqual({ key: "theme", value: "light" }));
    await user.click(screen.getByRole("button", { name: "System" }));
    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
  });

  it("general: translucent window background applies at once, asks the shell, and is saved", async () => {
    const user = userEvent.setup();
    const { backend } = await settings("general");
    expect(document.documentElement.hasAttribute("data-surface")).toBe(false);
    await user.click(await screen.findByRole("button", { name: "Translucent" }));
    await waitFor(() => expect(document.documentElement.getAttribute("data-surface")).toBe("translucent"));
    await waitFor(() => expect(backend.requests.some((r) => r.method === "desktop.window_surface" && (r.params as { translucent: boolean }).translucent)).toBe(true));
    await waitFor(() => expect(backend.requests.some((r) => r.method === "store.settings.set" && (r.params as { key: string; value: unknown }).value === "translucent")).toBe(true));
    expect(localStorage.getItem("marsad.surface")).toBe("translucent");
    const slider = screen.getByRole("slider", { name: "Translucency" });
    expect(slider).toHaveValue("50");
    expect(document.documentElement.style.getPropertyValue("--glass")).toBe("0.5");
    await user.click(screen.getByRole("button", { name: "Solid" }));
    await waitFor(() => expect(document.documentElement.hasAttribute("data-surface")).toBe(false));
    expect(screen.queryByRole("slider", { name: "Translucency" })).toBeNull();
  });

  it("general: a window that cannot be transparent stays solid and says why", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const request = backend.request.bind(backend);
    backend.request = (<T,>(method: string, params?: unknown, opts?: object) =>
      method === "desktop.window_surface" ? Promise.resolve({ transparent: false, effect: null } as T) : request<T>(method, params, opts)) as typeof backend.request;
    const r = await renderApp("/inbox", backend);
    act(() => r.history.push("/settings?s=general"));
    await user.click(await screen.findByRole("button", { name: "Translucent" }));
    await waitFor(() => expect(screen.getByTestId("surface-hint")).toHaveTextContent(/cannot show transparent windows/));
    expect(document.documentElement.hasAttribute("data-surface")).toBe(false);
  });

  it("notifications: the four event switches write store.notifications.set; sound and do-not-disturb are settings", async () => {
    const user = userEvent.setup();
    const { backend } = await settings("notifications");
    const sw = await screen.findByRole("switch", { name: /A session finished/ });
    expect(sw).toHaveAttribute("aria-checked", "true");
    await user.click(sw);
    await waitFor(() => expect(backend.requests.find((r) => r.method === "store.notifications.set")?.params).toEqual({ needs_you: true, failed: true, finished: false, host_lost: true, sign_in_needed: true }));
    expect(sw).toHaveAttribute("aria-checked", "false");
    await user.click(screen.getByRole("switch", { name: "Do not disturb" }));
    await waitFor(() => expect(backend.requests.some((r) => r.method === "store.settings.set" && (r.params as { key: string }).key === "notify.dnd")).toBe(true));
  });

  it("keyboard lists shortcuts of every screen, not only the mounted one", async () => {
    await settings("keyboard");
    expect(await screen.findByText("Review the waiting handoff")).toBeInTheDocument();
    expect(screen.getByText("Add host")).toBeInTheDocument();
  });

  it("about shows the daemon build", async () => {
    await settings("about");
    expect(await screen.findByText("fixture-0.1.0")).toBeInTheDocument();
    expect(within(screen.getByRole("navigation", { name: "Settings sections" })).getByRole("link", { name: "About" })).toHaveAttribute("aria-current", "page");
  });

  it("about shows the app's own version (not a made-up one) and, once connected, the daemon's build", async () => {
    await settings("about");
    expect(await screen.findByText("fixture-0.1.0")).toBeInTheDocument();
    expect(screen.getByTestId("app-version")).toHaveTextContent(`Marsad desktop ${pkg.version}`);
  });

  it("about does not show a daemon build it cannot have: without the link it says so", async () => {
    const backend = quietBackend();
    const r = await renderApp("/inbox", backend);
    act(() => r.history.push("/settings?s=about"));
    expect(await screen.findByText("fixture-0.1.0")).toBeInTheDocument();
    act(() => backend.setHubLink({ state: "reconnecting", error: "gone" }));
    expect(await screen.findByText("Not connected to the local service")).toBeInTheDocument();
    expect(screen.queryByText("fixture-0.1.0")).toBeNull();
    act(() => backend.setHubLink({ state: "connected" }));
    expect(await screen.findByText("fixture-0.1.0")).toBeInTheDocument();
  });

  it("there is no 'Play a sound' switch: nothing reads that setting", async () => {
    await settings("notifications");
    await screen.findByRole("switch", { name: "Do not disturb" });
    expect(screen.queryByText(/Play a sound/)).toBeNull();
    expect(screen.queryByRole("switch", { name: /sound/i })).toBeNull();
  });
});

describe("settings are what the hub has", () => {
  it("shows loading, not defaults, until the settings arrive", async () => {
    const backend = quietBackend();
    const h = hold(backend, "store.settings.get");
    const r = mountApp("/settings?s=general", backend);
    void r;
    expect(await screen.findByRole("status")).toHaveTextContent(/Loading your settings/);
    expect(screen.queryByRole("button", { name: "Light" })).toBeNull();
    expect(screen.queryByRole("slider")).toBeNull();
    await waitFor(() => expect(h.pending()).toBe(1));
    act(() => h.release());
    expect(await screen.findByRole("button", { name: "Light" })).toBeInTheDocument();
  });

  it("shows a failed load with its reason and a Try again that loads it", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const h = hold(backend, "store.settings.get");
    h.failAll({ code: "internal", message: "settings file is corrupt" });
    mountApp("/settings?s=general", backend);
    // (The link coming up asks again for what failed, so look at the settled state.)
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("settings file is corrupt"));
    expect(screen.getByText("Could not load your settings")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Light" })).toBeNull();
    h.healAll();
    await user.click(screen.getByRole("button", { name: "Try again" }));
    // While it is being asked again it says loading, not the old error and not defaults.
    expect(await screen.findByRole("status")).toHaveTextContent(/Loading your settings/);
    await waitFor(() => expect(h.pending()).toBe(1));
    act(() => h.release());
    expect(await screen.findByRole("button", { name: "Light" })).toBeInTheDocument();
    expect(screen.queryByText("Could not load your settings")).toBeNull();
  });

  it("a switch changes only when the hub's reply says so; it is busy meanwhile", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const r = await renderApp("/inbox", backend);
    act(() => r.history.push("/settings?s=notifications"));
    const dnd = await screen.findByRole("switch", { name: "Do not disturb" });
    expect(dnd).toHaveAttribute("aria-checked", "false");
    const h = hold(backend, "store.settings.set");
    await user.click(dnd);
    await waitFor(() => expect(h.pending()).toBe(1));
    expect(dnd).toHaveAttribute("aria-checked", "false");
    expect(dnd).toHaveAttribute("aria-busy", "true");
    expect(dnd).toBeDisabled();
    act(() => h.release());
    await waitFor(() => expect(dnd).toHaveAttribute("aria-checked", "true"));
    expect(dnd).not.toHaveAttribute("aria-busy");
  });

  it("a refused change leaves the switch at the stored value and shows the error", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const r = await renderApp("/inbox", backend);
    act(() => r.history.push("/settings?s=notifications"));
    const dnd = await screen.findByRole("switch", { name: "Do not disturb" });
    const h = hold(backend, "store.settings.set");
    await user.click(dnd);
    await waitFor(() => expect(h.pending()).toBe(1));
    act(() => h.reject({ code: "internal", message: "disk full" }));
    expect(await screen.findByText("disk full")).toBeInTheDocument();
    expect(screen.getByText("Could not save the setting")).toBeInTheDocument();
    expect(dnd).toHaveAttribute("aria-checked", "false");
    expect(dnd).toBeEnabled();
    expect(backend.hub.settings["notify.dnd"]).toBeUndefined();
  });

  it("notification choices are written from the reply too, one at a time", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const r = await renderApp("/inbox", backend);
    act(() => r.history.push("/settings?s=notifications"));
    const finished = await screen.findByRole("switch", { name: /A session finished/ });
    const failed = screen.getByRole("switch", { name: /A session failed/ });
    const h = hold(backend, "store.notifications.set");
    await user.click(finished);
    await waitFor(() => expect(h.pending()).toBe(1));
    expect(finished).toHaveAttribute("aria-checked", "true");
    // All four are busy: a second change now would send a stale copy of the other choices.
    expect(failed).toBeDisabled();
    act(() => h.reject({ code: "internal", message: "no space" }));
    expect(await screen.findByText("no space")).toBeInTheDocument();
    expect(finished).toHaveAttribute("aria-checked", "true");
    expect(backend.hub.notifications.finished).toBe(true);
    h.failAll(undefined);
    h.release();
    await user.click(finished);
    await waitFor(() => expect(h.pending()).toBe(1));
    act(() => h.release());
    await waitFor(() => expect(finished).toHaveAttribute("aria-checked", "false"));
    expect(backend.hub.notifications.finished).toBe(false);
  });

  it("without the link to the daemon the controls are off and say why (a change could not be saved)", async () => {
    const backend = quietBackend();
    const r = await renderApp("/inbox", backend);
    act(() => r.history.push("/settings?s=general"));
    const light = await screen.findByRole("button", { name: "Light" });
    act(() => backend.setHubLink({ state: "offline", error: "stopped" }));
    expect(await screen.findByTestId("settings-blocked")).toHaveTextContent(/Changes cannot be saved right now/);
    expect(light).toBeDisabled();
    act(() => backend.setHubLink({ state: "connected" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Light" })).toBeEnabled());
  });

  it("a theme that is painted at once is put back when the hub does not take it", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const r = await renderApp("/inbox", backend);
    act(() => r.history.push("/settings?s=general"));
    const light = await screen.findByRole("button", { name: "Light" });
    const h = hold(backend, "store.settings.set");
    await user.click(light);
    expect(document.documentElement.getAttribute("data-theme")).toBe("light");
    await waitFor(() => expect(h.pending()).toBe(1));
    act(() => h.reject({ code: "internal", message: "read-only" }));
    await screen.findByText("read-only");
    await waitFor(() => expect(document.documentElement.hasAttribute("data-theme")).toBe(false));
  });
});
