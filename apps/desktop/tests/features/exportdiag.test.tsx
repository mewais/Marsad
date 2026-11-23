import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import type { ExportStatus } from "../../src/bridge/wire";
import { toastStore } from "../../src/ui/toast";
import { hold, quietBackend, renderApp } from "../helpers";

type Backend = ReturnType<typeof quietBackend>;

/** The app on Settings → Diagnostics, reached from the Inbox, with exports played by hand unless a test says otherwise. */
async function open(backend: Backend = quietBackend(), manual = true) {
  backend.exports.manual = manual;
  const r = await renderApp("/inbox", backend);
  act(() => r.history.push("/settings?s=diagnostics"));
  await screen.findByRole("heading", { name: "Diagnostics" });
  return { ...r, backend, user: userEvent.setup() };
}
const card = () => screen.findByTestId("export-diagnostics");
const exportButton = async () => within(await card()).getByRole("button", { name: "Export" });
const step = (b: Backend, n = 1) => act(() => { for (let i = 0; i < n; i++) b.exports.step(); });
const rows = () => screen.getAllByTestId("export-source").map((r) => ({ name: r.querySelector("b")!.textContent, state: r.getAttribute("data-state"), text: r.textContent }));
const row = (name: string) => rows().find((r) => r.name === name)!;
const FIXED = { since_ms: 1_700_000_000_000, started_ms: 1_700_000_100_000 };
const status = (over: Partial<ExportStatus>): ExportStatus => ({ id: "fx900", state: "running", ...FIXED, sources: [], ...over });
const exports = (b: Backend) => b.requests.filter((r) => r.method === "diagnostics.export.start");

beforeEach(() => toastStore.set([]));

describe("Export diagnostics: the card", () => {
  it("is there without Developer mode, says what the file holds and never holds, and offers three windows with 24 hours chosen", async () => {
    const { backend } = await open();
    expect(backend.hub.settings["developer.mode"]).toBeUndefined();
    const c = await card();
    expect(within(c).getByText("Export diagnostics")).toBeInTheDocument();
    expect(c).toHaveTextContent(/connection steps, session states, timings and error codes recorded by this app, this computer and the hosts you are connected to/);
    expect(c).toHaveTextContent(/never contains prompts, replies, tool output, files or secrets/);
    const win = within(c).getByRole("group", { name: "Time window" });
    expect(within(win).getAllByRole("button").map((b) => [b.textContent, b.getAttribute("aria-pressed")])).toEqual([["Last 24 hours", "true"], ["Last 3 days", "false"], ["Last 7 days", "false"]]);
    await waitFor(async () => expect(await exportButton()).toBeEnabled());
    expect(await screen.findByTestId("export-none")).toHaveTextContent("No export has been made yet.");
  });

  it("does not offer Export until the hub has said whether one is running, and says it is asking", async () => {
    const backend = quietBackend();
    // The topic's first report is held back: the card knows nothing yet.
    const held: (() => void)[] = [];
    const subscribe = backend.subscribe.bind(backend);
    backend.subscribe = (topic, o, onEvent, onEnd) => subscribe(topic, o, topic === "diagnostics.exports" ? (e) => held.push(() => onEvent(e)) : onEvent, onEnd);
    await open(backend);
    expect(await screen.findByText("Asking the local service about exports…")).toBeInTheDocument();
    expect(await exportButton()).toBeDisabled();
    expect(await exportButton()).toHaveAttribute("title", "Waiting for the local service to report on exports.");
    expect(screen.queryByTestId("export-none")).toBeNull();
    await waitFor(() => expect(held).toHaveLength(1));
    act(() => held.splice(0).forEach((f) => f()));
    await waitFor(async () => expect(await exportButton()).toBeEnabled());
    expect(screen.queryByText("Asking the local service about exports…")).toBeNull();
  });

  it.each([["Last 24 hours", 24], ["Last 3 days", 72], ["Last 7 days", 168]])("%s asks for an absolute start that many hours back, with the trails", async (label, hours) => {
    const { backend, user } = await open();
    await user.click(within(await card()).getByRole("button", { name: label }));
    await waitFor(async () => expect(await exportButton()).toBeEnabled());
    const before = Date.now();
    await user.click(await exportButton());
    await waitFor(() => expect(exports(backend)).toHaveLength(1));
    const p = exports(backend)[0].params as { since_ms: number; include_trails: boolean };
    expect(p.include_trails).toBe(true);
    expect(p.since_ms).toBeGreaterThanOrEqual(before - hours * 3_600_000 - 50);
    expect(p.since_ms).toBeLessThanOrEqual(Date.now() - hours * 3_600_000 + 50);
  });

  it("starting is pending until the hub answers, then waits for the hub's first report instead of inventing progress", async () => {
    const { backend, user } = await open();
    await waitFor(async () => expect(await exportButton()).toBeEnabled());
    const start = hold(backend, "diagnostics.export.start");
    await user.click(await exportButton());
    await waitFor(async () => expect(await exportButton()).toHaveAttribute("aria-busy", "true"));
    expect(await exportButton()).toBeDisabled();
    expect(screen.queryByTestId("export-status")).toBeNull();
    start.release();
    const s = await screen.findByTestId("export-status");
    expect(s).toHaveAttribute("data-state", "running");
    expect(backend.diagEvents).toContainEqual({ ev: "ui.export", stage: "start" });
  });

  it("shows each source as the hub reports it, in plain words, and offers no second export while one runs", async () => {
    const { backend, user } = await open();
    await waitFor(async () => expect(await exportButton()).toBeEnabled());
    await user.click(await exportButton());
    const s = await screen.findByTestId("export-status");
    expect(s).toHaveTextContent("Exporting…");
    expect(s).toHaveTextContent("0 of 6 sources finished");
    expect(rows().map((r) => [r.name, r.state])).toEqual([["This app", "waiting"], ["This computer", "waiting"], ["hetzner-ax41", "waiting"], ["win-build", "waiting"], ["gpu-box", "waiting"], ["pi-runner", "waiting"]]);
    expect(row("This app").text).toContain("Waiting");
    // The app is read: reading, with the bytes so far; then done.
    step(backend);
    await waitFor(() => expect(row("This app").state).toBe("reading"));
    expect(row("This app").text).toMatch(/Reading.*4\.1 KB/);
    step(backend);
    await waitFor(() => expect(row("This app").state).toBe("done"));
    expect(row("This app").text).toMatch(/Done.*18 KB/);
    // A second export is not offered while one runs.
    expect(await exportButton()).toBeDisabled();
    expect(await exportButton()).toHaveAttribute("title", "An export is already running.");
    expect(screen.getByText("Another export cannot start until this one ends.")).toBeInTheDocument();
    // This computer is read whole, the host is read in part with the reason, the others were not connected, each with why.
    step(backend, 4);
    await waitFor(() => expect(row("hetzner-ax41").state).toBe("partial"));
    expect(row("This computer").state).toBe("done");
    expect(row("hetzner-ax41").text).toMatch(/Only partly read.*the host's read budget ran out: 3 older sections were left out/);
    step(backend, 3);
    await waitFor(() => expect(row("win-build").state).toBe("not_connected"));
    expect(row("win-build").text).toMatch(/Not connected.*reconnecting \(attempt 3\): connection reset by peer/);
    expect(row("gpu-box").text).toMatch(/Not connected.*waiting for you to sign in/);
    expect(row("pi-runner").text).toMatch(/Not connected.*installed or updated/);
    expect(screen.getByTestId("export-status")).toHaveTextContent("6 of 6 sources finished");
  });

  it("when it is done: the file's size, that parts were left out, and Save… with the file's name", async () => {
    const { backend, user } = await open();
    await waitFor(async () => expect(await exportButton()).toBeEnabled());
    await user.click(await exportButton());
    await screen.findByTestId("export-status");
    step(backend, 100);
    await waitFor(() => expect(screen.getByTestId("export-status")).toHaveAttribute("data-state", "done"));
    const done = backend.exports.list[0];
    const result = screen.getByTestId("export-result");
    expect(result).toHaveAttribute("data-complete", "false");
    expect(result).toHaveTextContent(/Some parts were left out\. The file lists what is missing and why\./);
    expect(result.textContent).toMatch(/\d+(\.\d)? KB/);
    expect(await exportButton()).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "Save…" }));
    await waitFor(() => expect(backend.requests.filter((r) => r.method === "desktop.export_save").map((r) => r.params)).toEqual([{ id: done.file }]));
    expect(await screen.findByTestId("export-saved")).toHaveTextContent(`/home/me/Downloads/${done.file}`);
    expect(backend.diagEvents.filter((e) => e.ev === "ui.export")).toEqual([{ ev: "ui.export", stage: "start" }, { ev: "ui.export", stage: "saved" }]);
    // After saving: Show in folder, through the shell's reveal.
    await user.click(screen.getByRole("button", { name: "Show in folder" }));
    await waitFor(() => expect(backend.requests.find((r) => r.method === "desktop.reveal")?.params).toEqual({ target: `/home/me/Downloads/${done.file}`, distro: null }));
  });

  it("an export with nothing left out says so", async () => {
    const backend = quietBackend();
    backend.exports.list = [status({ state: "done", sources: [{ source: "app", state: "done", bytes: 9000 }], file: "e.ndjson.gz", bytes: 2048, complete: true })];
    await open(backend);
    const result = await screen.findByTestId("export-result");
    expect(result).toHaveAttribute("data-complete", "true");
    expect(result).toHaveTextContent("2.0 KB · Everything planned is in the file.");
  });

  it("an export whose completeness the hub did not state is not called complete", async () => {
    const backend = quietBackend();
    backend.exports.list = [status({ state: "done", sources: [], file: "e.ndjson.gz", bytes: 100 })];
    await open(backend);
    expect(await screen.findByTestId("export-result")).toHaveTextContent("The hub did not say whether the file is complete.");
  });

  it("closing the save dialog is not an error: nothing is saved, nothing is shown as failed, and it is recorded as cancelled", async () => {
    const backend = quietBackend();
    backend.exports.list = [status({ state: "done", sources: [], file: "e.ndjson.gz", bytes: 100, complete: true })];
    backend.exportSaveAnswer = () => ({ saved: false });
    const { user } = await open(backend);
    await user.click(await screen.findByRole("button", { name: "Save…" }));
    expect(await screen.findByTestId("export-cancelled")).toHaveTextContent("Not saved");
    expect(screen.queryByRole("alert")).toBeNull();
    expect(toastStore.get()).toEqual([]);
    expect(screen.queryByRole("button", { name: "Show in folder" })).toBeNull();
    expect(backend.diagEvents).toEqual([{ ev: "ui.export", stage: "save_cancelled" }]);
    // It can be saved after all.
    backend.exportSaveAnswer = (id) => ({ path: `/tmp/${id}` });
    await user.click(screen.getByRole("button", { name: "Save…" }));
    expect(await screen.findByTestId("export-saved")).toHaveTextContent("/tmp/e.ndjson.gz");
  });

  it("a save that fails shows the reason and its code is recorded; Save… can be tried again", async () => {
    const backend = quietBackend();
    backend.exports.list = [status({ state: "done", sources: [], file: "e.ndjson.gz", bytes: 100, complete: true })];
    const { user } = await open(backend);
    const save = hold(backend, "desktop.export_save");
    save.failAll({ code: "not_found", message: "no such export" });
    await user.click(await screen.findByRole("button", { name: "Save…" }));
    const problem = await screen.findByRole("alert");
    expect(problem).toHaveTextContent("Could not save the file.");
    expect(problem).toHaveTextContent("no such export");
    expect(backend.diagEvents).toEqual([{ ev: "ui.export", stage: "failed", code: "not_found" }]);
    expect(screen.getByRole("button", { name: "Save…" })).toBeEnabled();
  });

  it("a failed export shows its reason and Try again starts another with the window chosen", async () => {
    const backend = quietBackend();
    backend.exports.list = [status({ state: "failed", sources: [{ source: "app", state: "failed", bytes: 0, reason: "the app's log could not be read" }], error: "the export directory is not writable" })];
    const { user } = await open(backend);
    expect(await screen.findByTestId("export-failure")).toHaveTextContent("the export directory is not writable");
    expect(row("This app").text).toContain("the app's log could not be read");
    expect(screen.queryByRole("button", { name: "Save…" })).toBeNull();
    await user.click(within(await card()).getByRole("button", { name: "Last 7 days" }));
    await user.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(exports(backend)).toHaveLength(1));
    expect((exports(backend)[0].params as { since_ms: number }).since_ms).toBeLessThan(Date.now() - 6 * 24 * 3_600_000);
    expect((await screen.findByTestId("export-status"))).toHaveAttribute("data-state", "running");
  });

  it("an export that failed without a reason says the hub gave none", async () => {
    const backend = quietBackend();
    backend.exports.list = [status({ state: "failed" })];
    await open(backend);
    expect(await screen.findByTestId("export-failure")).toHaveTextContent("The hub gave no reason.");
  });

  it("an export that is already running when the page opens is shown, and Export is not offered", async () => {
    const backend = quietBackend();
    backend.exports.list = [status({ sources: [{ source: "app", state: "reading", bytes: 2048 }, { source: "host:hz", host: "hz", state: "waiting", bytes: 0 }] })];
    await open(backend);
    const s = await screen.findByTestId("export-status");
    expect(s).toHaveAttribute("data-state", "running");
    expect(s).toHaveTextContent("0 of 2 sources finished");
    expect(rows().map((r) => r.name)).toEqual(["This app", "hetzner-ax41"]);
    expect(await exportButton()).toBeDisabled();
    // The hub's next report moves it on.
    act(() => backend.exports.upsert(status({ state: "done", sources: [{ source: "app", state: "done", bytes: 9000 }], file: "f.ndjson.gz", bytes: 1500, complete: true })));
    await screen.findByTestId("export-result");
    expect(await exportButton()).toBeEnabled();
  });

  it("an export that finished before the page opened is shown with its result; with several, the newest", async () => {
    const backend = quietBackend();
    backend.exports.list = [
      status({ id: "old", state: "done", started_ms: 1000, sources: [], file: "old.ndjson.gz", bytes: 10, complete: true }),
      status({ id: "new", state: "done", started_ms: 5000, sources: [], file: "new.ndjson.gz", bytes: 20, complete: false }),
    ];
    const { user } = await open(backend);
    expect(await screen.findByTestId("export-result")).toHaveAttribute("data-complete", "false");
    await user.click(screen.getByRole("button", { name: "Save…" }));
    await waitFor(() => expect(backend.requests.find((r) => r.method === "desktop.export_save")?.params).toEqual({ id: "new.ndjson.gz" }));
  });

  it("if the hub refuses with a conflict anyway, the reason is shown", async () => {
    const backend = quietBackend();
    const { user } = await open(backend);
    await waitFor(async () => expect(await exportButton()).toBeEnabled());
    // Another client started one just now: the hub refuses this app's request.
    backend.exports.refuse = { code: "conflict", message: "an export is already running (fx777); wait for it to end" };
    await user.click(await exportButton());
    const refused = await screen.findByTestId("export-refused");
    expect(refused).toHaveTextContent("Could not start the export.");
    expect(refused).toHaveTextContent("an export is already running (fx777); wait for it to end");
    expect(backend.diagEvents).toContainEqual({ ev: "ui.export", stage: "failed", code: "conflict" });
    // Once the hub reports that one runs, it is shown; when it ends, the refusal is gone.
    backend.exports.refuse = null;
    act(() => backend.exports.upsert(status({ id: "fx777", sources: [{ source: "app", state: "reading", bytes: 1 }] })));
    expect(await screen.findByTestId("export-status")).toHaveAttribute("data-state", "running");
    expect(screen.getByTestId("export-refused")).toBeInTheDocument();
    act(() => backend.exports.upsert(status({ id: "fx777", state: "done", sources: [], file: "x.ndjson.gz", bytes: 5, complete: true })));
    await waitFor(() => expect(screen.queryByTestId("export-refused")).toBeNull());
  });

  it("plays by itself when nothing steps it, from waiting to a finished file", async () => {
    const backend = quietBackend();
    const { user } = await open(backend, false);
    await waitFor(async () => expect(await exportButton()).toBeEnabled());
    await user.click(await exportButton());
    expect(await screen.findByTestId("export-result", {}, { timeout: 5000 })).toBeInTheDocument();
    expect(rows().map((r) => r.state)).toEqual(["done", "done", "partial", "not_connected", "not_connected", "not_connected"]);
  });
});

describe("Export diagnostics: the hub", () => {
  it("is disabled with the reason while the local service is unreachable, and says where the app's own log still is", async () => {
    const backend = quietBackend();
    await open(backend);
    await waitFor(async () => expect(await exportButton()).toBeEnabled());
    act(() => backend.setHubLink({ state: "offline", error: "the daemon socket is gone" }));
    const down = await screen.findByTestId("export-unreachable");
    expect(down).toHaveTextContent("An export cannot be made now.");
    expect(down).toHaveTextContent(/the daemon socket is gone\. The app's own log file/);
    expect(down).toHaveTextContent("diag/app.ndjson");
    expect(within(await card()).queryByRole("button", { name: "Export" })).toBeNull();
    expect(backend.subscriptions.filter((s) => s.topic === "diagnostics.exports")).toEqual([]);
    expect(exports(backend)).toEqual([]);
    // Back: the card reads the hub again and is usable.
    act(() => backend.setHubLink({ state: "connected" }));
    await waitFor(async () => expect(await exportButton()).toBeEnabled());
  });

  it("is usable when settings could not be loaded: it needs the hub, not the settings", async () => {
    const backend = quietBackend();
    const get = hold(backend, "store.settings.get");
    get.failAll({ code: "io", message: "settings unreadable" });
    await open(backend);
    expect(await screen.findByText(/Could not load your settings/)).toBeInTheDocument();
    await waitFor(async () => expect(await exportButton()).toBeEnabled());
  });

  it("holds the topic open only while the card is open", async () => {
    const backend = quietBackend();
    const { history } = await open(backend);
    await waitFor(() => expect(backend.subscriptions.filter((s) => s.topic === "diagnostics.exports")).toHaveLength(1));
    act(() => history.push("/inbox"));
    await waitFor(() => expect(backend.subscriptions.filter((s) => s.topic === "diagnostics.exports")).toEqual([]));
  });

  it("an ended subscription is shown with its reason and can be started again; Export and Save are off meanwhile", async () => {
    const backend = quietBackend();
    backend.exports.list = [status({ state: "done", sources: [], file: "e.ndjson.gz", bytes: 100, complete: true })];
    const { user } = await open(backend);
    await screen.findByTestId("export-result");
    act(() => backend.endSubscriptions("diagnostics.exports", { code: "not_hub", message: "this daemon is not a hub" }));
    const problem = await screen.findByText("Export progress stopped.");
    expect(problem.closest("[role=alert]")).toHaveTextContent("this daemon is not a hub");
    expect(await exportButton()).toBeDisabled();
    expect(screen.getByRole("button", { name: "Save…" })).toBeDisabled();
    await user.click(within(await card()).getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(screen.queryByText("Export progress stopped.")).toBeNull());
    await waitFor(async () => expect(await exportButton()).toBeEnabled());
    expect(backend.subscriptions.filter((s) => s.topic === "diagnostics.exports")).toHaveLength(1);
  });
});
