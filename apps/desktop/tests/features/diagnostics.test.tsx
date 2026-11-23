import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { linesOf } from "../../src/bridge/fixture/diagnostics";
import { toastStore } from "../../src/ui/toast";
import { hold, quietBackend, renderApp, sessionByName } from "../helpers";

type Backend = ReturnType<typeof quietBackend>;

/** The app on `path`, reached from the Inbox the way a user would (these pages show no session rows to wait for). */
async function openPage(path: string, backend = quietBackend()) {
  const r = await renderApp("/inbox", backend);
  act(() => r.history.push(path));
  return { ...r, backend };
}

/** The app on a session's page (`tab`), with Developer mode as the hub has it. */
async function openSession(name: string, tab: string, o: { developer?: boolean } = {}) {
  const backend = quietBackend();
  if (o.developer ?? true) backend.hub.settings["developer.mode"] = true;
  const s = sessionByName(backend, name);
  const r = await renderApp(`/session/${s.host}/${s.id}${tab === "activity" ? "" : `?tab=${tab}`}`, backend);
  await screen.findByRole("heading", { name });
  return { ...r, s, backend };
}

const opened = (backend: Backend) => backend.subscriptions.filter((x) => x.topic.startsWith("session.trail:") || x.topic === "diag.tail");
const panel = () => screen.findByTestId("session-diagnostics");

describe("settings: Developer mode", () => {
  const settings = (backend = quietBackend()) => openPage("/settings?s=diagnostics", backend);

  it("is off until the hub says it is on, and says it is loading until then", async () => {
    const backend = quietBackend();
    const get = hold(backend, "store.settings.get");
    await openPage("/settings?s=diagnostics", backend);
    expect(await screen.findByText(/Loading your settings/)).toBeInTheDocument();
    expect(screen.queryByRole("switch", { name: "Developer mode" })).toBeNull();
    get.release();
    expect(await screen.findByRole("switch", { name: "Developer mode" })).toHaveAttribute("aria-checked", "false");
  });

  it("is a switch in a Diagnostics section, with one sentence of what it shows, and a place for the export card that draws nothing", async () => {
    await settings();
    expect(await screen.findByRole("heading", { name: "Diagnostics" })).toBeInTheDocument();
    const sw = screen.getByRole("switch", { name: "Developer mode" });
    expect(sw).toHaveAttribute("aria-checked", "false");
    expect(screen.getByText(/Shows a Diagnostics tab on every session and a connection log on every host/)).toBeInTheDocument();
    const slot = document.querySelector('[data-slot="export-diagnostics"]') as HTMLElement;
    expect(slot).not.toBeNull();
    expect(slot.hidden).toBe(true);
    expect(slot.querySelector("button, a, input")).toBeNull();
    // The only control in the section is the switch.
    expect(Array.from(document.querySelectorAll(".setbody button, .setbody input, .setbody [role=switch]")).map((e) => e.getAttribute("aria-label"))).toEqual(["Developer mode"]);
  });

  it("turning it on is pending until the hub has it, is stored under developer.mode, and is recorded by the app afterwards", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    await settings(backend);
    const set = hold(backend, "store.settings.set");
    const sw = await screen.findByRole("switch", { name: "Developer mode" });
    await user.click(sw);
    await waitFor(() => expect(sw).toHaveAttribute("aria-busy", "true"));
    expect(sw).toBeDisabled();
    expect(sw).toHaveAttribute("aria-checked", "false");
    expect(backend.diagEvents).toEqual([]);
    set.release();
    await waitFor(() => expect(sw).toHaveAttribute("aria-checked", "true"));
    expect(sw).not.toHaveAttribute("aria-busy");
    expect(backend.requests.find((r) => r.method === "store.settings.set")?.params).toEqual({ key: "developer.mode", value: true });
    expect(backend.hub.settings["developer.mode"]).toBe(true);
    await waitFor(() => expect(backend.diagEvents).toEqual([{ ev: "ui.developer_mode", on: true }]));
  });

  it("when the hub refuses, the switch stays as it was, the reason is shown, and nothing is recorded", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    toastStore.set([]);
    await settings(backend);
    const set = hold(backend, "store.settings.set");
    set.failAll({ code: "io", message: "the settings file is read-only" });
    const sw = await screen.findByRole("switch", { name: "Developer mode" });
    await user.click(sw);
    await waitFor(() => expect(toastStore.get().some((t) => t.title === "Could not save the setting" && t.detail === "the settings file is read-only")).toBe(true));
    expect(sw).toHaveAttribute("aria-checked", "false");
    expect(backend.diagEvents).toEqual([]);
  });

  it("turning it off again is stored as off and recorded", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    backend.hub.settings["developer.mode"] = true;
    await settings(backend);
    const sw = await screen.findByRole("switch", { name: "Developer mode" });
    expect(sw).toHaveAttribute("aria-checked", "true");
    await user.click(sw);
    await waitFor(() => expect(sw).toHaveAttribute("aria-checked", "false"));
    await waitFor(() => expect(backend.diagEvents).toEqual([{ ev: "ui.developer_mode", on: false }]));
  });
});

describe("developer mode off: no diagnostics anywhere", () => {
  it("a session has no Diagnostics tab, and an address that names it opens the first one", async () => {
    const { backend } = await openSession("docs-search-index", "diagnostics", { developer: false });
    const tabs = within(screen.getByRole("tablist", { name: "Session sections" }));
    expect(tabs.getAllByRole("tab").map((t) => t.textContent)).toEqual(["Activity", "Changes", "Config", "Terminal"]);
    expect(tabs.getByRole("tab", { name: "Activity" })).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByTestId("session-diagnostics")).toBeNull();
    expect(opened(backend)).toEqual([]);
  });

  it("a host shows no Diagnostics section and asks nothing of any log", async () => {
    const { backend } = await openPage("/hosts?h=win");
    await screen.findByRole("complementary", { name: "win-build details" });
    expect(screen.queryByTestId("host-diagnostics")).toBeNull();
    expect(opened(backend)).toEqual([]);
  });
});

describe("a session's Diagnostics tab", () => {
  it("appears with Developer mode, holds the trail open only while it is the visible tab, and is gone again when the mode is switched off", async () => {
    const user = userEvent.setup();
    const { backend, history, s } = await openSession("docs-search-index", "activity");
    const tabs = screen.getByRole("tablist", { name: "Session sections" });
    expect(within(tabs).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Activity", "Changes", "Config", "Terminal", "Diagnostics"]);
    // A closed panel costs nothing on any machine.
    expect(opened(backend)).toEqual([]);
    await user.click(within(tabs).getByRole("tab", { name: "Diagnostics" }));
    await panel();
    await waitFor(() => expect(opened(backend).map((x) => x.topic)).toEqual([`session.trail:${s.id}`]));
    expect(opened(backend)[0].host).toBe(s.host);
    await user.click(within(tabs).getByRole("tab", { name: "Activity" }));
    await waitFor(() => expect(opened(backend)).toEqual([]));
    // Back in, then Developer mode goes off while the panel is open.
    await user.click(within(tabs).getByRole("tab", { name: "Diagnostics" }));
    await waitFor(() => expect(opened(backend)).toHaveLength(1));
    await act(async () => {
      await backend.request("store.settings.set", { key: "developer.mode", value: null });
    });
    // The page reads the setting from the app's store, which the hub's reply to a change updates.
    act(() => history.push(`/settings?s=diagnostics`));
    const sw = await screen.findByRole("switch", { name: "Developer mode" });
    expect(sw).toHaveAttribute("aria-checked", "true"); // the app's store still has what the hub told it
    await user.click(sw);
    await waitFor(() => expect(sw).toHaveAttribute("aria-checked", "false"));
    act(() => history.push(`/session/${s.host}/${s.id}?tab=diagnostics`));
    await screen.findByRole("heading", { name: "docs-search-index" });
    expect(screen.queryByRole("tab", { name: "Diagnostics" })).toBeNull();
    expect(screen.queryByTestId("session-diagnostics")).toBeNull();
    expect(opened(backend)).toEqual([]);
  });

  it("the tab disappears while it is open when Developer mode is turned off, and the subscriptions go with it", async () => {
    const user = userEvent.setup();
    const { backend, s, history } = await openSession("docs-search-index", "diagnostics");
    await panel();
    await waitFor(() => expect(opened(backend)).toHaveLength(1));
    // Turn it off from Settings in another window's way: through the app's own control, then come back.
    act(() => history.push("/settings?s=diagnostics"));
    await user.click(await screen.findByRole("switch", { name: "Developer mode" }));
    await waitFor(() => expect(opened(backend)).toEqual([]));
    act(() => history.push(`/session/${s.host}/${s.id}?tab=diagnostics`));
    await screen.findByRole("heading", { name: "docs-search-index" });
    expect(screen.getByRole("tab", { name: "Activity" })).toHaveAttribute("aria-selected", "true");
  });

  it("shows the session's facts, each copyable, and says what the daemon has not reported", async () => {
    const user = userEvent.setup({ writeToClipboard: true });
    const { s, backend } = await openSession("docs-search-index", "diagnostics");
    const p = within(await panel());
    const facts = Object.fromEntries(Array.from(p.getByText("Session id").closest("dl")!.children).map((row) => [row.querySelector("dt")!.textContent, row.querySelector("dd")!.textContent]));
    expect(facts).toMatchObject({ "Session id": s.id, Host: "thinkpad-x1 (x1)", Driver: "opencode", Generation: "1", "Native session id": "native-9", Phase: "idle", "CLI version": "1.3.2", Model: "anthropic/claude-sonnet-4.5" });
    expect(p.getAllByRole("button", { name: /^Copy (session id|host|driver|generation|native session id|phase|cli version|model)$/i })).toHaveLength(8);
    await user.click(p.getByRole("button", { name: "Copy session id" }));
    expect(await navigator.clipboard.readText()).toBe(s.id);
    void backend;
  });

  it("the trail is a timeline in the host's order, each entry a sentence that opens to its raw fields; a long trail pages back", async () => {
    const user = userEvent.setup();
    const { backend, s } = await openSession("upgrade-tauri-2-5", "diagnostics");
    const p = within(await panel());
    const list = await p.findByRole("list", { name: "Session trail" });
    await waitFor(() => expect(within(list).getAllByRole("listitem").length).toBeGreaterThan(10));
    // The newest 200 entries first; older ones on request. The list starts at its bottom: the newest entry is the last row.
    const rows = within(list).getAllByRole("listitem");
    expect(rows[rows.length - 1]).toHaveTextContent("The provider reports a rate limit: allowed_warning (five_hour)");
    expect(rows[rows.length - 2]).toHaveTextContent("The conversation was compacted: 168400 → 41200 tokens in 9.4 s (auto)");
    const times = rows.map((r) => Number(r.getAttribute("data-t")));
    expect(times).toEqual([...times].sort((a, b) => a - b));
    await user.click(p.getByRole("button", { name: "Load earlier" }));
    await waitFor(() => expect(p.queryByRole("button", { name: "Load earlier" })).toBeNull());
    expect(backend.requests.filter((r) => r.method === "session.trail.page").map((r) => (r.params as { before?: number }).before !== undefined)).toEqual([false, true]);
    // A row opens to the entry's raw fields.
    const row = within(list).getAllByRole("listitem")[0];
    await user.click(within(row).getByRole("button"));
    expect(within(row).getAllByRole("term").map((t) => t.textContent)).toEqual(expect.arrayContaining(["cursor", "time", "ev"]));
    await user.click(within(row).getByRole("button"));
    expect(within(row).queryAllByRole("term")).toEqual([]);
    expect(s.id).toBeTruthy();
  });

  it("the story of a session shows in words: a prompt dropped by an interrupt, and output after the turn had ended", async () => {
    await openSession("upgrade-tauri-2-5", "diagnostics");
    const p = within(await panel());
    const prompts = await p.findAllByTestId("prompt-row");
    const dropped = prompts.filter((r) => r.querySelector('[data-step="dropped"]'));
    expect(dropped.map((r) => r.getAttribute("data-input"))).toEqual(["640", "641"]);
    const row = within(dropped[0]);
    expect(row.getByText("Dropped (interrupted)")).toBeInTheDocument();
    // Steps nothing saw are hollow and say so, never guessed.
    expect(dropped[0].querySelector('[data-step="app"]')).toHaveAttribute("data-state", "not_seen");
    expect(dropped[0].querySelector('[data-step="recorded"]')).toHaveAttribute("data-state", "seen");
    expect(dropped[0].querySelector('[data-step="written"]')).toBeNull();
    const list = await p.findByRole("list", { name: "Session trail" });
    expect(await within(list).findByText("The agent produced output after its turn had ended (first: tool_call_started)")).toBeInTheDocument();
    expect(within(list).getByText("Idle — the turn was interrupted")).toBeInTheDocument();
    expect(within(list).getAllByText(/will never reach the agent — it was interrupted while it waited/)).toHaveLength(2);
  });

  it("a failed session says why in the timeline", async () => {
    await openSession("ci-triage-lead", "diagnostics");
    const list = await within(await panel()).findByRole("list", { name: "Session trail" });
    expect(await within(list).findByText("Failed — the process exited with code 1: pnpm test exited 1")).toBeInTheDocument();
    expect(within(list).getByText("The process exited with code 1")).toBeInTheDocument();
  });

  it("a prompt sent from the app shows its whole path: the app, the hub, the host's daemon and the host's own record; the other machine's times are approximate", async () => {
    const user = userEvent.setup();
    const { backend, history, s } = await openSession("sqlite-wal-tuning", "activity");
    await user.type(await screen.findByRole("textbox", { name: "Message" }), `a prompt{Enter}`);
    await waitFor(() => expect(backend.requests.some((r) => r.method === "session.command" && r.trace)).toBe(true));
    const trace = backend.requests.find((r) => r.method === "session.command")!.trace!;
    act(() => history.push(`/session/${s.host}/${s.id}?tab=diagnostics`));
    const rowOf = async () => (await within(await panel()).findAllByTestId("prompt-row")).find((r) => r.textContent!.includes(trace))!;
    await waitFor(async () => expect((await rowOf())?.querySelector('[data-step="acknowledged"]')).toHaveAttribute("data-state", "seen"));
    const row = await rowOf();
    expect(Array.from(row.querySelectorAll("[data-step]")).map((e) => `${e.getAttribute("data-step")}:${e.getAttribute("data-state")}`)).toEqual([
      "app:seen", "hub:seen", "host:seen", "recorded:seen", "sent:seen", "written:seen", "acknowledged:seen", "turn:seen",
    ]);
    expect(within(row).getByText(trace)).toBeInTheDocument();
    // The app and the hub are this computer; the host is another machine, so its times carry "≈".
    const times = Array.from(row.querySelectorAll("[data-step]")).map((e) => e.querySelector(".dgp-time")!.textContent!);
    expect(times[0]).toBe("start");
    expect(times[1]).not.toMatch(/≈/);
    expect(times.slice(2).every((t) => t.startsWith("≈"))).toBe(true);
    expect(within(row).getByText(/≈ means another machine's clock/)).toBeInTheDocument();
    expect(Array.from(row.querySelectorAll("[data-step] .dgp-src")).map((e) => e.textContent)).toEqual(["app", "hub", "hetzner-ax41", "hetzner-ax41", "hetzner-ax41", "hetzner-ax41", "hetzner-ax41", "hetzner-ax41"]);
    // The app asked the hub's daemon and the host's for this trace only, on the daemons they belong to.
    const tails = backend.subscriptions.filter((x) => x.topic === "diag.tail");
    expect(tails.map((t) => [t.host ?? "hub", (t.params as { traces: string[] }).traces])).toEqual(expect.arrayContaining([["hub", [trace]], ["hz", [trace]]]));
  });

  it("a prompt to the local host has no separate host step and no approximate times", async () => {
    const user = userEvent.setup();
    const { backend, history, s } = await openSession("docs-search-index", "activity");
    await user.type(await screen.findByRole("textbox", { name: "Message" }), `a prompt{Enter}`);
    await waitFor(() => expect(backend.requests.some((r) => r.method === "session.command" && r.trace)).toBe(true));
    act(() => history.push(`/session/${s.host}/${s.id}?tab=diagnostics`));
    const trace = backend.requests.find((r) => r.method === "session.command")!.trace!;
    const rowOf = async () => (await within(await panel()).findAllByTestId("prompt-row")).find((r) => r.textContent!.includes(trace))!;
    await waitFor(async () => expect((await rowOf())?.querySelector('[data-step="hub"]')).toHaveAttribute("data-state", "seen"));
    const row = await rowOf();
    expect(row.querySelector('[data-step="host"]')).toBeNull();
    expect(within(row).queryByText(/≈/)).toBeNull();
    expect(backend.subscriptions.filter((x) => x.topic === "diag.tail").every((t) => t.host === undefined)).toBe(true);
  });

  it("a prompt the host refused is a row that ends at Refused with the host's reason", async () => {
    const user = userEvent.setup();
    const { backend, history, s } = await openSession("docs-search-index", "activity");
    await user.type(await screen.findByRole("textbox", { name: "Message" }), `first{Enter}`);
    await waitFor(() => expect(backend.requests.filter((r) => r.method === "session.command")).toHaveLength(1));
    await backend.request("sessions.stop", { id: s.id }, { host: s.host });
    await waitFor(() => expect(backend.snapshot(s.host, s.id).phase.state).toBe("exited"));
    // The composer is off for a finished session: the refusal is sent as the app would send it.
    const trace = "u-00000000000000aa";
    backend.requests.length = 0;
    await expect(backend.request("session.command", { id: s.id, command: { command: "prompt", text: "late" } }, { host: s.host, trace })).rejects.toMatchObject({ code: "not_running" });
    act(() => history.push(`/session/${s.host}/${s.id}?tab=diagnostics`));
    await within(await panel()).findAllByTestId("prompt-row");
    // The app did not record this one (it did not send it): what the daemons saw is not a prompt row of this panel.
    expect(linesOf(backend.hubLog).some((l) => l.includes(`"session.command_refused"`) && l.includes(trace))).toBe(true);
  });

  it("the host's log for the session is closed until opened; opening asks the host's daemon for this session only", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    backend.hub.settings["developer.mode"] = true;
    const id = backend.sessionIds("hz")[0];
    await renderApp(`/session/hz/${id}?tab=diagnostics`, backend);
    const p = within(await panel());
    await waitFor(() => expect(opened(backend).map((x) => x.topic)).toEqual([`session.trail:${id}`]));
    const toggle = p.getByRole("button", { name: /Host log for this session/ });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(opened(backend).some((x) => x.topic === "diag.tail")).toBe(false);
    await user.click(toggle);
    const log = await p.findByRole("list", { name: "Host log for this session" });
    expect(opened(backend).find((x) => x.topic === "diag.tail")).toMatchObject({ host: "hz", params: { session: id } });
    expect(within(log).getByText(new RegExp(`Session ${id} lost its process`))).toBeInTheDocument();
    expect(within(log).getByText(new RegExp(`Reconnecting to the process of session ${id} — attempt 1`))).toBeInTheDocument();
    await user.click(toggle);
    await waitFor(() => expect(opened(backend).some((x) => x.topic === "diag.tail")).toBe(false));
  });

  it("a host that is not connected is said so with the reason, nothing is asked of it, and it reads by itself when it is back", async () => {
    const { backend, s } = await openSession("windows-print-check", "diagnostics");
    const un = await screen.findByTestId("diag-unreachable");
    expect(un).toHaveTextContent("win-build is not connected (reconnecting).");
    expect(un).toHaveTextContent("reads again by itself");
    expect(opened(backend)).toEqual([]);
    act(() => backend.setLink("win", { state: "connected", hello: { protocol: { major: 1, minor: 0 }, build: "fixture", host_id: "win", role: "daemon", features: [] } }));
    await waitFor(() => expect(opened(backend).map((x) => x.topic)).toEqual([`session.trail:${s.id}`]));
    expect(screen.queryByTestId("diag-unreachable")).toBeNull();
  });

  it("an unreachable host offers to connect again; once it is connected the panel reads by itself", async () => {
    const user = userEvent.setup();
    const { backend, s } = await openSession("windows-print-check", "diagnostics");
    const un = await screen.findByTestId("diag-unreachable");
    await user.click(within(un).getByRole("button", { name: "Reconnect to win-build" }));
    await waitFor(() => expect(backend.requests.find((r) => r.method === "hosts.reconnect")?.params).toEqual({ id: "win" }));
    await waitFor(() => expect(opened(backend).map((x) => x.topic)).toEqual([`session.trail:${s.id}`]));
    expect(screen.queryByTestId("diag-unreachable")).toBeNull();
  });

  it("a live trail that ends is shown with its reason and starts again on request", async () => {
    const user = userEvent.setup();
    const { backend, s } = await openSession("docs-search-index", "diagnostics");
    await panel();
    await waitFor(() => expect(opened(backend)).toHaveLength(1));
    act(() => backend.endSubscriptions(`session.trail:${s.id}`, { code: "lagged", message: "too slow to keep up" }));
    const alert = await screen.findByText("The live trail stopped.");
    expect(alert.closest('[role="alert"]')).toHaveTextContent("too slow to keep up");
    expect(opened(backend)).toEqual([]);
    await user.click(within(alert.closest('[role="alert"]') as HTMLElement).getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(opened(backend)).toHaveLength(1));
    expect(screen.queryByText("The live trail stopped.")).toBeNull();
  });

  it("a trail that cannot be read says why, and a retry reads it", async () => {
    const backend = quietBackend();
    backend.hub.settings["developer.mode"] = true;
    const request = backend.request.bind(backend);
    let fail = true;
    backend.request = (<T,>(m: string, p?: unknown, o?: object) => (fail && m === "session.trail.page" ? Promise.reject({ code: "timeout", message: "hz did not answer in time" }) : request<T>(m, p, o))) as typeof backend.request;
    const s = sessionByName(backend, "docs-search-index");
    await renderApp(`/session/${s.host}/${s.id}?tab=diagnostics`, backend);
    const alert = await screen.findByText("Could not read the trail.");
    expect(alert.closest('[role="alert"]')).toHaveTextContent("hz did not answer in time");
    fail = false;
    await userEvent.setup().click(within(alert.closest('[role="alert"]') as HTMLElement).getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("list", { name: "Session trail" })).toBeInTheDocument();
    expect(screen.queryByText("Could not read the trail.")).toBeNull();
  });

  it("while the trail is being read it says so; the UI is not blocked", async () => {
    const backend = quietBackend();
    backend.hub.settings["developer.mode"] = true;
    const h = hold(backend, "session.trail.page");
    const s = sessionByName(backend, "docs-search-index");
    await renderApp(`/session/${s.host}/${s.id}?tab=diagnostics`, backend);
    expect(await screen.findByText("Reading the trail from the host…")).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Activity" })).toBeEnabled();
    h.release();
    expect(await screen.findByRole("list", { name: "Session trail" })).toBeInTheDocument();
    expect(screen.queryByText("Reading the trail from the host…")).toBeNull();
  });

  describe("copy as text", () => {
    const writeText = vi.fn();
    beforeEach(() => {
      writeText.mockReset().mockResolvedValue(undefined);
      Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    });
    afterEach(() => vi.restoreAllMocks());

    it("puts the whole panel on the clipboard as text: facts, paths, timeline", async () => {
      const user = userEvent.setup({ writeToClipboard: false });
      Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
      await openSession("upgrade-tauri-2-5", "diagnostics");
      const p = within(await panel());
      await p.findByRole("list", { name: "Session trail" });
      await user.click(p.getByRole("button", { name: "Copy as text" }));
      await waitFor(() => expect(writeText).toHaveBeenCalled());
      const text = writeText.mock.calls[0][0] as string;
      expect(text).toContain("Marsad session diagnostics");
      expect(text).toContain("Native session id: native-10");
      expect(text).toContain("#640 prompt, 58 characters");
      expect(text).toContain("Dropped (interrupted)");
      expect(text).toContain("The agent produced output after its turn had ended");
      expect(text).toContain("(older entries exist and are not included)");
      expect(text).toContain("Host log for this session\n  not opened");
      expect(await p.findByRole("button", { name: "Copied" })).toBeInTheDocument();
    });
  });

  it("never shows anything from the transcript: its text is not in the panel", async () => {
    const { backend } = await openSession("upgrade-tauri-2-5", "diagnostics");
    const p = await panel();
    await within(p).findByRole("list", { name: "Session trail" });
    const s = sessionByName(backend, "upgrade-tauri-2-5");
    const words = ["Work on upgrade-tauri-2-5", "Looking at the surrounding code", "Starting on", "src-tauri/capabilities", "module-3.ts"];
    for (const w of words) expect(p.textContent).not.toContain(w);
    expect(p.textContent).not.toContain(s.snap.title === null ? "never" : `First prompt`);
  });
});

describe("a host's Diagnostics section", () => {
  async function openHost(id: string, backend = quietBackend()) {
    backend.hub.settings["developer.mode"] = true;
    const { history } = await openPage(`/hosts?h=${id}`, backend);
    const aside = await screen.findByRole("complementary", { name: /details$/ });
    return { backend, history, aside, section: () => within(aside).findByTestId("host-diagnostics") };
  }

  it("shows the connection log by attempt: the steps, the durations, the outcome, and why a connect failed with the stage, exit code and the host's scrubbed words", async () => {
    const { backend, section } = await openHost("win");
    const log = within(await within(await section()).findByTestId("connection-log"));
    await waitFor(() => expect(log.getAllByTestId("attempt").length).toBe(6));
    const attempts = log.getAllByTestId("attempt");
    expect(attempts.map((a) => a.getAttribute("data-attempt"))).toEqual(["1", "between", "2", "between", "3", "between"]);
    const a3 = attempts.find((a) => a.getAttribute("data-attempt") === "3")!;
    const t = within(a3);
    expect(t.getByText("Attempt 3")).toBeInTheDocument();
    expect(t.getByText("Failed, will retry")).toBeInTheDocument();
    expect(a3).toHaveTextContent("10 s");
    expect(a3).toHaveTextContent("Failed at spawn (unreachable) · exit code 255");
    expect(t.getByLabelText("What the host's shell wrote (scrubbed)")).toHaveTextContent("ssh: connect to host win-build.lan port 22: Connection reset by peer");
    expect(a3).toHaveTextContent("Started ssh for win (attempt 3; 9 arguments, pid 21003)");
    // Only this host's records: no other host's attempt is in it.
    expect(log.queryByText(/\bhz\b/)).toBeNull();
    // It asked the hub, once, and for nothing else.
    expect(backend.subscriptions.filter((x) => x.topic === "diag.tail").map((x) => x.host ?? "hub")).toEqual(["hub"]);
  });

  it("the first attempt that connected is there too, with its steps and how long each took", async () => {
    const { section } = await openHost("hz");
    const log = within(await within(await section()).findByTestId("connection-log"));
    const a1 = await waitFor(() => log.getAllByTestId("attempt")[0]);
    expect(a1).toHaveAttribute("data-attempt", "1");
    expect(within(a1).getByText("Connected")).toBeInTheDocument();
    expect(a1).toHaveTextContent("The host reports linux/x86_64, helper 0.7.3 (372 ms)");
    expect(a1).toHaveTextContent("The helper answered (build 0.7.3) in 44 ms");
    expect(a1).toHaveTextContent("Attempt 1 connected after 482 ms");
  });

  it("a host that asks for a sign-in shows the sign-in and its outcome among the steps", async () => {
    const { section } = await openHost("gpu");
    const log = within(await within(await section()).findByTestId("connection-log"));
    const a1 = await waitFor(() => log.getAllByTestId("attempt")[0]);
    expect(a1).toHaveTextContent("Sign-in prompt 1 (secret), attempt 1");
    expect(a1).toHaveTextContent("Sign-in prompt 1 was cancelled: you cancelled it");
    expect(a1).toHaveTextContent("Failed at spawn (auth_needed) · exit code 255");
    expect(within(a1).getByText("Failed, waiting for you")).toBeInTheDocument();
  });

  it("a connected host also shows what its own daemon says about itself, and only that kind of record", async () => {
    const { backend, section } = await openHost("hz");
    const on = await within(await section()).findByRole("region", { name: "On the host" });
    const list = await within(on).findByRole("list", { name: "On the host" });
    expect(opened(backend).find((x) => x.host === "hz")).toMatchObject({ topic: "diag.tail", host: "hz" });
    const text = list.textContent!;
    expect(text).toContain("Daemon started (host hz, build 0.7.3");
    expect(text).toContain("Daemon ready in 231 ms");
    expect(text).toContain("lost its process");
    expect(text).toContain("Reconnecting to the process of session");
    // The refused command is a session record, not one of this section's kinds.
    expect(text).not.toContain("was refused before it was recorded");
  });

  it("a host that is not connected has no 'On the host' section and asks nothing of it", async () => {
    const { backend, section } = await openHost("win");
    const s = within(await section());
    expect(s.queryByRole("region", { name: "On the host" })).toBeNull();
    expect(s.getByText(/win-build's own log is read while it is connected/)).toBeInTheDocument();
    await s.findByTestId("connection-log");
    expect(backend.subscriptions.some((x) => x.host === "win")).toBe(false);
  });

  it("the local host has no link: it says so and shows the hub daemon's own log", async () => {
    const { section, backend } = await openHost("x1");
    const s = within(await section());
    expect(s.getByRole("note")).toHaveTextContent("This is the local host: this app reaches it through the local service (the hub), so there is no connection to log. This is the hub daemon's own log.");
    expect(s.queryByTestId("connection-log")).toBeNull();
    const list = await s.findByRole("list", { name: "Local service log" });
    expect(list.textContent).toContain("Daemon started (hub x1, build fixture-0.1.0");
    expect(backend.subscriptions.filter((x) => x.topic === "diag.tail")).toEqual([{ topic: "diag.tail", host: undefined, params: {} }]);
  });

  it("holds nothing open when the section is not on screen, and closes what it opened when another host is chosen", async () => {
    const user = userEvent.setup();
    const { backend, aside, history } = await openHost("win");
    await within(aside).findByTestId("connection-log");
    await waitFor(() => expect(opened(backend)).toHaveLength(1));
    act(() => history.push("/hosts?h=hz"));
    await waitFor(() => expect(within(screen.getByRole("complementary", { name: "hetzner-ax41 details" })).getByRole("region", { name: "On the host" })).toBeInTheDocument());
    await waitFor(() => expect(opened(backend).map((x) => `${x.topic}@${x.host ?? "hub"}`).sort()).toEqual(["diag.tail@hub", "diag.tail@hz"]));
    // Leaving the page closes everything.
    await user.click(screen.getByRole("link", { name: /Inbox/ }));
    await waitFor(() => expect(opened(backend)).toEqual([]));
  });

  it("a hub link that is down is said so and nothing is asked", async () => {
    const backend = quietBackend();
    backend.hub.settings["developer.mode"] = true;
    await openPage("/hosts?h=win", backend);
    await screen.findByRole("complementary", { name: "win-build details" });
    await within(screen.getByRole("complementary", { name: "win-build details" })).findByTestId("connection-log");
    act(() => backend.setHubLink({ state: "reconnecting", error: "the local service stopped" }));
    const un = await within(screen.getByRole("complementary", { name: "win-build details" })).findByTestId("diag-unreachable");
    expect(un).toHaveTextContent("the local service stopped");
    await waitFor(() => expect(opened(backend)).toEqual([]));
    await userEvent.setup().click(within(un).getByRole("button", { name: "Reconnect to the local service" }));
    expect(backend.hubReconnects).toBe(1);
    await waitFor(() => expect(opened(backend)).toHaveLength(1));
    expect(screen.queryByTestId("diag-unreachable")).toBeNull();
  });

  it("a log that ends is shown with its reason and read again on request", async () => {
    const user = userEvent.setup();
    const { backend, aside } = await openHost("win");
    const log = within(await within(aside).findByTestId("connection-log"));
    await waitFor(() => expect(opened(backend)).toHaveLength(1));
    act(() => backend.endSubscriptions("diag.tail", { code: "lagged", message: "the hub log fell behind" }));
    const alert = await log.findByRole("alert");
    expect(alert).toHaveTextContent("The connection log stopped.");
    expect(alert).toHaveTextContent("the hub log fell behind");
    await user.click(within(alert).getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(opened(backend)).toHaveLength(1));
    expect(log.queryByRole("alert")).toBeNull();
    // It resumed where it was: the attempts are still there, once each.
    expect(log.getAllByTestId("attempt").map((a) => a.getAttribute("data-attempt"))).toEqual(["1", "between", "2", "between", "3", "between"]);
  });
});
