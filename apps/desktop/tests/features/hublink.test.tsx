import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { mountApp, quietBackend, renderApp, sessionByName } from "../helpers";

const banner = () => screen.queryByTestId("connection-banner");
const rows = () => [...document.querySelectorAll<HTMLElement>("[data-session]")];
const hubChip = () => within(screen.getByRole("contentinfo", { name: "Status" })).getByText(/thinkpad-x1/).closest(".sb-item") as HTMLElement;

describe("the app's own link to its local daemon", () => {
  it("while it is still connecting, says so instead of promising hosts; nothing is invented", async () => {
    const backend = quietBackend();
    backend.setHubLink({ state: "connecting" });
    mountApp("/inbox", backend);
    const b = await screen.findByTestId("connection-banner");
    expect(b).toHaveAttribute("data-hub", "connecting");
    expect(b).toHaveTextContent(/Connecting to the local service/);
    expect(b).toHaveTextContent(/starting it if it is not running/i);
    expect(within(b).queryByRole("button")).toBeNull();
    expect(await screen.findByRole("heading", { name: "Connecting to the local service" })).toBeInTheDocument();
    expect(screen.queryByText("Connecting to your hosts")).toBeNull();
    expect(rows()).toHaveLength(0);
    // Nothing is reported yet, so no zeros stand in for it, and no toast complains about the link.
    expect(screen.getByText(/Waiting for the first report from the local service/)).toBeInTheDocument();
    expect(screen.queryByText(/0 need you/)).toBeNull();
    expect(screen.getByRole("contentinfo", { name: "Status" })).toHaveTextContent("Today: not reported yet");
    expect(document.querySelector(".toast")).toBeNull();
    act(() => backend.setHubLink({ state: "connected" }));
    await waitFor(() => expect(rows().length).toBeGreaterThan(0));
    expect(banner()).toBeNull();
    expect(hubChip()).not.toHaveTextContent(/connecting/i);
  });

  it("a daemon that could not be started shows why, says Marsad will not retry, and Try again calls the shell", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    backend.setHubLink({ state: "offline", error: "cannot start marsad-agent: No such file or directory" });
    mountApp("/inbox", backend);
    const b = await screen.findByTestId("connection-banner");
    expect(b).toHaveAttribute("role", "alert");
    expect(b).toHaveTextContent("Cannot reach the local service");
    expect(b).toHaveTextContent("cannot start marsad-agent: No such file or directory");
    expect(b).toHaveTextContent(/does not retry this on its own/);
    expect(await screen.findByRole("heading", { name: "Cannot reach the local service" })).toBeInTheDocument();
    await user.click(within(b).getByRole("button", { name: "Try again" }));
    expect(backend.hubReconnects).toBe(1);
    await waitFor(() => expect(rows().length).toBeGreaterThan(0));
    expect(banner()).toBeNull();
  });

  it("when it drops, what is on screen is marked last known, counted apart, and actions that need the daemon are off with the reason", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    await renderApp("/inbox", backend);
    const live = rows().filter((r) => r.dataset.stale === undefined && r.dataset.status === "needs");
    expect(live.length).toBeGreaterThan(0);
    const badge = () => within(screen.getByRole("navigation", { name: "Main" })).queryByLabelText(/need you/);
    expect(badge()).not.toBeNull();
    expect(document.querySelectorAll(".ibx-item .st.working svg.spin").length).toBeGreaterThan(0);

    act(() => backend.setHubLink({ state: "reconnecting", error: "connection refused (os error 111)" }));
    const b = await screen.findByTestId("connection-banner");
    expect(b).toHaveAttribute("data-hub", "reconnecting");
    expect(b).toHaveTextContent("connection refused (os error 111)");
    expect(b).toHaveTextContent(/last known state/);
    // Every row is last known: dashed pill, no spinner, a caption saying so.
    await waitFor(() => expect(rows().every((r) => r.dataset.stale !== undefined)).toBe(true));
    expect(document.querySelectorAll(".ibx-item .st svg.spin")).toHaveLength(0);
    expect(document.querySelectorAll(".ibx-item .st.stale").length).toBe(rows().length);
    expect(screen.getAllByText("last known").length).toBeGreaterThan(0);
    // Nothing is counted as live activity: no needs-you badge, no bell badge, the summary says how many are last known.
    expect(badge()).toBeNull();
    expect(screen.getByTestId("summary-stale")).toHaveTextContent(String(rows().length));
    // Daemon-dependent actions are disabled and say why; reading still works.
    const markAll = screen.getAllByRole("button", { name: "Mark all seen" });
    for (const m of markAll) {
      expect(m).toBeDisabled();
      expect(m).toHaveAttribute("title", expect.stringMatching(/Reconnecting to the local service: connection refused/));
    }
    await user.click(markAll[0]);
    expect(backend.requests.some((r) => r.method === "store.seen.mark_all")).toBe(false);
    // The local host's own chip is not "Connected".
    expect(hubChip()).toHaveTextContent(/reconnecting/i);

    act(() => backend.setHubLink({ state: "connected" }));
    await waitFor(() => expect(banner()).toBeNull());
    for (const m of screen.getAllByRole("button", { name: "Mark all seen" })) expect(m).toBeEnabled();
    expect(badge()).not.toBeNull();
    expect(screen.queryByTestId("summary-stale")?.textContent ?? "").not.toMatch(new RegExp(`^\\s*${rows().length}\\b`));
  });

  it("the Hosts screen shows the local host's real link, with the reason and a Try again that reconnects", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const r = await renderApp("/inbox", backend);
    act(() => backend.setHubLink({ state: "offline", error: "the daemon exited: exit status 1" }));
    act(() => r.history.push("/hosts?h=x1"));
    const detail = await screen.findByRole("complementary", { name: "thinkpad-x1 details" });
    expect(detail).toHaveTextContent("Cannot reach the local service");
    expect(detail).toHaveTextContent("the daemon exited: exit status 1");
    const row = document.querySelector('[data-host="x1"]') as HTMLElement;
    expect(within(row).getByText(/Not connected/)).toBeInTheDocument();
    await user.click(within(detail).getByRole("button", { name: "Try again" }));
    expect(backend.hubReconnects).toBe(1);
    await waitFor(() => expect(within(row).getByText("Connected")).toBeInTheDocument());
  });

  it("a request made without the daemon is not sent and does not wait: it fails at once with the reason", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const s = sessionByName(backend, "docs-search-index");
    const r = mountApp(`/session/${s.host}/${s.id}`, backend);
    void r;
    await screen.findByRole("heading", { name: "docs-search-index" });
    const box = await screen.findByRole("textbox", { name: "Message" });
    act(() => backend.setHubLink({ state: "offline", error: "gone" }));
    await screen.findByTestId("composer-blocked");
    await user.type(box, "hello");
    expect(screen.getByRole("button", { name: /Send/ })).toBeDisabled();
    await user.type(box, "{Enter}");
    expect(box).toHaveValue("hello");
    expect(backend.requests.some((q) => q.method === "session.command" && (q.params as { command: { command: string } }).command.command === "prompt")).toBe(false);
    expect(screen.getByRole("button", { name: /Interrupt/ })).toBeDisabled();
    expect(screen.getByTestId("composer-blocked")).toHaveTextContent(/Cannot reach the local service: gone/);
  });
});

describe("a subscription that ends", () => {
  it("is reported (with the daemon's reason), what it fed is last known, and Try again starts it again", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    await renderApp("/inbox", backend);
    act(() => backend.endSubscriptions("fleet"));
    const b = await screen.findByTestId("connection-banner");
    expect(b).toHaveAttribute("role", "alert");
    expect(b).toHaveTextContent("Live updates stopped for hosts and sessions");
    expect(b).toHaveTextContent("this daemon is not a hub");
    // The link to the daemon is fine, yet nothing says the sessions are live any more.
    expect(b).toHaveAttribute("data-hub", "connected");
    await waitFor(() => expect(rows().every((r) => r.dataset.stale !== undefined)).toBe(true));
    await user.click(within(b).getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(banner()).toBeNull());
    await waitFor(() => expect(rows().some((r) => r.dataset.stale === undefined)).toBe(true));
  });

  it("starts again by itself when the link to the daemon comes back (no timers: the event drives it)", async () => {
    const backend = quietBackend();
    await renderApp("/inbox", backend);
    act(() => backend.endSubscriptions("fleet", null));
    expect(await screen.findByTestId("connection-banner")).toHaveTextContent("the daemon closed it");
    act(() => backend.setHubLink({ state: "reconnecting", error: "restarting" }));
    expect(screen.getByTestId("connection-banner")).toHaveAttribute("data-hub", "reconnecting");
    act(() => backend.setHubLink({ state: "connected" }));
    await waitFor(() => expect(banner()).toBeNull());
    await waitFor(() => expect(rows().some((r) => r.dataset.stale === undefined)).toBe(true));
  });

  it("a session's transcript resumes from where it was after its subscription is started again", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const s = sessionByName(backend, "docs-search-index");
    await renderApp(`/session/${s.host}/${s.id}`, backend);
    await screen.findByRole("heading", { name: "docs-search-index" });
    await screen.findByRole("textbox", { name: "Message" });
    act(() => backend.endSubscriptions(`session.events:${s.id}`));
    const b = await screen.findByTestId("connection-banner");
    expect(b).toHaveTextContent("Live updates stopped for a session's transcript");
    // A message that arrives while the subscription is down is caught up on when it is started again.
    await backend.request("session.command", { id: s.id, command: { command: "prompt", text: "while it was down" } }, { host: s.host });
    expect(screen.queryByText("while it was down")).toBeNull();
    await user.click(within(b).getByRole("button", { name: "Try again" }));
    expect(await screen.findByText("while it was down")).toBeInTheDocument();
    expect(banner()).toBeNull();
  });
});
