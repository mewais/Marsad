import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { quietBackend, renderApp } from "../helpers";

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
    await waitFor(() => expect(backend.requests.find((r) => r.method === "store.notifications.set")?.params).toEqual({ needs_you: true, failed: true, finished: false, host_lost: true }));
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
});
