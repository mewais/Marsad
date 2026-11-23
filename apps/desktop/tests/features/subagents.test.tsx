import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { BACKGROUND_SPEC } from "../../src/bridge/fixture/background";
import type { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import { SPECS } from "../../src/bridge/fixture/sessions";
import { sessionPath } from "../../src/router";
import { hold, quietBackend, renderApp, sessionByName } from "../helpers";

/** The fixture's background scenario: a billing subagent and a test run still going, an invoice subagent that reported back. */
const backgroundBackend = () => quietBackend({ specs: [...SPECS, BACKGROUND_SPEC] });
const NAME = "route-auth-audit";

type Ref = ReturnType<typeof sessionByName>;

async function open(backend: FixtureBackend = backgroundBackend(), name = NAME, path?: (s: Ref) => string) {
  const s = sessionByName(backend, name);
  const r = await renderApp(path?.(s) ?? `/session/${s.host}/${s.id}`, backend);
  await screen.findByRole("heading", { name });
  return { ...r, s };
}
const snap = (b: FixtureBackend, s: Ref) => b.snapshot(s.host, s.id);
const commands = (b: FixtureBackend) => b.requests.filter((r) => r.method === "session.command").map((r) => (r.params as { command: { command: string; task_id?: string } }).command);
const pill = () => document.querySelector(".sd-title .st")!;
const work = (b: FixtureBackend, s: Ref) => b.work(s.host, s.id);
const BILLING = { call_id: "c-bill", agent_id: "agent-bill-1" };

describe("the main transcript is the session's own conversation", () => {
  it("shows no line of a subagent, and one entry per subagent with the state the daemon reports", async () => {
    await open();
    await screen.findByText(/All three are running/);
    // Their own lines are not the session's.
    expect(screen.queryByText(/I'll read each invoice route/)).toBeNull();
    expect(screen.queryByText(/The billing module has webhook handlers/)).toBeNull();
    const cards = await screen.findAllByTestId("subagent");
    expect(cards).toHaveLength(2);
    const [billing, invoice] = cards;
    // The tool calls that started them returned at once; the billing subagent is still running because its task has not ended.
    expect(billing).toHaveTextContent("Subagent · Explore");
    expect(billing).toHaveTextContent("Audit the billing routes");
    expect(within(billing).getByText("Running")).toBeInTheDocument();
    expect(within(billing).getByTestId("subagent-meta")).toHaveTextContent("3 tool calls");
    expect(within(invoice).getByText("Finished")).toBeInTheDocument();
    expect(within(invoice).getByTestId("subagent-meta")).toHaveTextContent("2 tool calls");
  });

  it("a subagent is running until the daemon says its task ended, however long ago its call returned", async () => {
    const backend = backgroundBackend();
    const { s } = await open(backend);
    const card = () => screen.getAllByTestId("subagent")[0];
    await waitFor(() => expect(within(card()).getByText("Running")).toBeInTheDocument());
    act(() => work(backend, s).endTask("task-bill", "completed", "Billing is guarded."));
    await waitFor(() => expect(within(card()).getByText("Finished")).toBeInTheDocument());
    expect(snap(backend, s).subagents.find((x) => x.call_id === "c-bill")).toMatchObject({ running: false, ok: true, status: "completed" });
  });

  it("an ended task the agent reports as not completed shows the agent's word", async () => {
    const backend = backgroundBackend();
    const { s } = await open(backend);
    act(() => work(backend, s).endTask("task-bill", "stopped"));
    await waitFor(() => expect(within(screen.getAllByTestId("subagent")[0]).getByText("Stopped")).toBeInTheDocument());
  });

  it("says where background work started and ended, and the turn the agent started by itself with the agent's own reason", async () => {
    await open();
    expect(await screen.findByText("Background task started · local_agent · “Audit the billing routes”")).toBeInTheDocument();
    expect(screen.getByText("Background task started · local_bash · “pnpm test --run”")).toBeInTheDocument();
    expect(screen.getByText(/Background task ended · local_agent · “Audit the invoice routes” · completed — Both invoice routes are guarded/)).toBeInTheDocument();
    expect(screen.getByText("The agent started a turn on its own · origin: task-notification (session-task) · Audit the invoice routes")).toBeInTheDocument();
  });
});

describe("Activity lists the subagents; selecting one opens its transcript", () => {
  it("lists them running first, then finished; each row says its state, tool calls and time", async () => {
    await open();
    const list = await screen.findByTestId("side-subagents");
    const rows = within(list).getAllByTestId("side-subagent");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent(/Explore.*Audit the billing routes/);
    expect(rows[0]).toHaveTextContent("Running");
    expect(rows[0]).toHaveTextContent("3 tool calls");
    expect(rows[1]).toHaveTextContent(/Explore.*Audit the invoice routes/);
    expect(rows[1]).toHaveTextContent("Finished");
    // The shell command is a background task, not a subagent: it has its own list.
    const tasks = screen.getByTestId("side-tasks");
    expect(within(tasks).getAllByTestId("side-task")).toHaveLength(1);
    expect(tasks).toHaveTextContent("pnpm test --run");
    expect(tasks).toHaveTextContent("local_bash");
  });

  it("opens the subagent's own transcript at an address, read only and saying whose it is; Back returns to the session", async () => {
    const user = userEvent.setup();
    const { s, history } = await open();
    const row = within(await screen.findByTestId("side-subagents")).getAllByTestId("side-subagent")[0];
    await user.click(within(row).getByRole("link"));
    expect(history.store.get()).toBe(sessionPath(s.host, s.id, undefined, "c-bill"));
    expect(history.store.get()).toBe(`/session/${s.host}/${s.id}?sub=c-bill`);
    // Its conversation, not the session's.
    expect(await screen.findByText(/The billing module has webhook handlers/)).toBeInTheDocument();
    expect(screen.queryByText(/All three are running/)).toBeNull();
    expect(screen.queryByText(/I'll read each invoice route/)).toBeNull();
    // Read only: no composer, and the page says whose transcript this is.
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull();
    expect(screen.getByTestId("subagent-readonly")).toHaveTextContent("This is the transcript of the subagent “Audit the billing routes” (Explore). It is read only");
    expect(screen.getByTestId("subagent-bar")).toHaveTextContent("Subagent · Explore");
    expect(screen.getByTestId("subagent-bar")).toHaveTextContent("Running");
    // The row it was opened from is marked.
    expect(within(await screen.findByTestId("side-subagents")).getAllByRole("link")[0]).toHaveAttribute("aria-current", "page");
    await user.click(screen.getByTestId("subagent-back"));
    expect(history.store.get()).toBe(`/session/${s.host}/${s.id}`);
    expect(await screen.findByText(/All three are running/)).toBeInTheDocument();
    expect(screen.queryByText(/The billing module has webhook handlers/)).toBeNull();
    expect(screen.getByRole("textbox", { name: "Message" })).toBeInTheDocument();
  });

  it("the card in the transcript opens it too", async () => {
    const user = userEvent.setup();
    const { s, history } = await open();
    await user.click((await screen.findAllByTestId("subagent-open"))[1]);
    expect(history.store.get()).toBe(`/session/${s.host}/${s.id}?sub=c-inv`);
    expect(await screen.findByText(/I'll read each invoice route/)).toBeInTheDocument();
    // Its transcript starts with its task and ends with the agent's account of it.
    expect(screen.getByText(/Background task ended · local_agent · “Audit the invoice routes” · completed/)).toBeInTheDocument();
  });

  it("a subagent's address can be opened directly, and Esc goes back to the session before it leaves it", async () => {
    const user = userEvent.setup();
    const backend = backgroundBackend();
    const { s, history } = await open(backend, NAME, (x) => `/session/${x.host}/${x.id}?sub=c-bill`);
    expect(await screen.findByText(/The billing module has webhook handlers/)).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(history.store.get()).toBe(`/session/${s.host}/${s.id}`));
    await user.keyboard("{Escape}");
    await waitFor(() => expect(history.store.get()).toBe("/inbox"));
    expect(commands(backend)).toEqual([]);
  });

  it("a subagent the session has no record of says so and still shows what the daemon has of it", async () => {
    await open(backgroundBackend(), NAME, (x) => `/session/${x.host}/${x.id}?sub=c-unknown`);
    expect(await screen.findByTestId("subagent-readonly")).toHaveTextContent("This session has no record of that subagent.");
    expect(await screen.findByTestId("transcript-empty")).toHaveTextContent("Nothing from a subagent yet.");
  });

  it("stays live from the session's one subscription: its new lines appear there, never in the session's own transcript", async () => {
    const user = userEvent.setup();
    const backend = backgroundBackend();
    const { s } = await open(backend, NAME, (x) => `/session/${x.host}/${x.id}?sub=c-bill`);
    await screen.findByText(/The billing module has webhook handlers/);
    act(() => work(backend, s).scoped(BILLING, { type: "assistant_delta", text: "Webhook handlers verify a signature, not a session.", message_id: "b9" }));
    expect(await screen.findByText("Webhook handlers verify a signature, not a session.")).toBeInTheDocument();
    // One subscription on the session's events, whatever is open.
    expect(backend.subscriptions.filter((x) => x.topic.startsWith("session.events:"))).toHaveLength(1);
    await user.click(screen.getByTestId("subagent-back"));
    await screen.findByText(/All three are running/);
    expect(screen.queryByText("Webhook handlers verify a signature, not a session.")).toBeNull();
    expect(backend.subscriptions.filter((x) => x.topic.startsWith("session.events:"))).toHaveLength(1);
  });

  it("is loaded with the daemon's scoped history, a page at a time, and 'Load earlier' pages within the scope", async () => {
    const user = userEvent.setup();
    const backend = backgroundBackend();
    const s0 = sessionByName(backend, NAME);
    // The subagent speaks 70 more times: more than a page.
    for (let i = 0; i < 70; i++) work(backend, s0).scoped(BILLING, { type: "assistant_delta", text: `Billing finding number ${i + 1}.`, message_id: `f${i}` });
    const { s } = await open(backend, NAME, (x) => `/session/${x.host}/${x.id}?sub=c-bill`);
    await screen.findByText("Billing finding number 70.");
    expect(screen.queryByText("Billing finding number 1.")).toBeNull();
    const histories = backend.requests.filter((r) => r.method === "session.history").map((r) => r.params as { scope?: { kind: string; call_id?: string }; before?: number });
    expect(histories.map((h) => h.scope)).toEqual(expect.arrayContaining([{ kind: "main" }, { kind: "subagent", call_id: "c-bill" }]));
    await user.click(await screen.findByRole("button", { name: "Load earlier messages" }));
    await screen.findByText(/The billing module has webhook handlers/);
    const earlier = backend.requests.filter((r) => r.method === "session.history").map((r) => r.params as { scope?: { kind: string; call_id?: string }; before?: number }).filter((h) => h.before !== undefined);
    expect(earlier).toHaveLength(1);
    expect(earlier[0].scope).toEqual({ kind: "subagent", call_id: "c-bill" });
    void s;
  });
});

describe("background work is visible, and the status stays the daemon's", () => {
  it("the session pill and header say it is working in the background; the Inbox row says why", async () => {
    const backend = backgroundBackend();
    const { s } = await open(backend);
    expect(snap(backend, s).phase.state).toBe("working");
    expect(snap(backend, s).turn ?? null).toBeNull();
    expect(pill()).toHaveTextContent("Working in the background: 2 tasks");
    expect(pill()).toHaveAttribute("data-status", "working");
  });

  it("a turn open next to background tasks: the pill stays Working and the header counts the tasks", async () => {
    const backend = quietBackend();
    const { s } = await open(backend, "csv-schema-worker"); // working: a turn is open
    act(() => work(backend, s).startTask({ task_id: "t-a", kind: "local_bash", description: "pnpm build", call_id: "c-a" }));
    await waitFor(() => expect(screen.getByTestId("background-running")).toHaveTextContent("1 background task running"));
    expect(pill()).toHaveTextContent("Working");
    expect(pill()).not.toHaveTextContent("background");
  });

  it("the Inbox row of such a session says why in its line", async () => {
    const backend = backgroundBackend();
    const s = sessionByName(backend, NAME);
    await renderApp("/inbox", backend);
    const row = await waitFor(() => {
      const r = document.querySelector<HTMLElement>(`[data-session="${s.key}"]`);
      if (!r) throw new Error("row not there");
      return r;
    });
    expect(row).toHaveAttribute("data-status", "working");
    expect(row).toHaveTextContent("Working in the background: 2 tasks");
  });

  it("the end of the last task is Idle, and a turn the agent then starts by itself is Working and noticed; its end says why it ran", async () => {
    const backend = backgroundBackend();
    const { s } = await open(backend);
    act(() => {
      work(backend, s).endTask("task-bill", "completed", "Billing is guarded.");
      work(backend, s).endTask("task-suite", "completed", "52 passed");
    });
    // Between the end of the work and the agent's own turn the session is Idle, as the daemon says.
    await waitFor(() => expect(pill()).toHaveTextContent("Idle"));
    expect(screen.getByRole("button", { name: "Interrupt" })).toBeDisabled();
    act(() => work(backend, s).startOwnTurn());
    await waitFor(() => expect(pill()).toHaveTextContent("Working"));
    expect(pill()).not.toHaveTextContent("background");
    expect(screen.getByRole("button", { name: "Interrupt" })).toBeEnabled();
    expect(await screen.findByText("The agent started a turn on its own")).toBeInTheDocument();
    act(() => work(backend, s).finishOwnTurn("All three audits are done.", { kind: "task-notification", run_id: "run-bill" }));
    await waitFor(() => expect(pill()).toHaveTextContent("Idle"));
    expect(await screen.findAllByText(/The agent started a turn on its own · origin: task-notification · Audit the billing routes/)).toHaveLength(1);
    expect(snap(backend, s).last_turn?.origin).toMatchObject({ kind: "task-notification" });
  });
});

describe("stopping work", () => {
  it("Interrupt with background work only is enabled and says what it stops; it stops the tasks, and the session is Idle", async () => {
    const user = userEvent.setup();
    const backend = backgroundBackend();
    const { s } = await open(backend);
    const button = screen.getByRole("button", { name: "Interrupt" });
    expect(button).toBeEnabled();
    expect(button).toHaveAttribute("title", "No turn is running. Stop the 2 background tasks; the agent stays open.");
    await user.click(button);
    await waitFor(() => expect(snap(backend, s).phase.state).toBe("idle"));
    expect(commands(backend)).toEqual([{ command: "interrupt" }]);
    expect(snap(backend, s).tasks.filter((t) => t.running)).toHaveLength(0);
    expect(snap(backend, s).tasks.map((t) => t.status)).toEqual(expect.arrayContaining(["stopped"]));
    await waitFor(() => expect(pill()).toHaveTextContent("Idle"));
  });

  it("Esc does not stop background work: with no turn open it goes back, and sends nothing", async () => {
    const user = userEvent.setup();
    const backend = backgroundBackend();
    const { s, history } = await open(backend);
    await user.keyboard("{Escape}");
    await waitFor(() => expect(history.store.get()).toBe("/inbox"));
    expect(commands(backend)).toEqual([]);
    expect(snap(backend, s).background_running).toBe(2);
  });

  it("Esc with a turn running stops that turn and leaves the background tasks alone", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { s } = await open(backend, "csv-schema-worker");
    act(() => work(backend, s).startTask({ task_id: "t-a", kind: "local_bash", description: "pnpm build", call_id: "c-a" }));
    await waitFor(() => expect(screen.getByTestId("background-running")).toBeInTheDocument());
    await user.keyboard("{Escape}");
    await waitFor(() => expect(commands(backend)).toEqual([{ command: "interrupt" }]));
    // The turn is over; the task still runs, so the session is still Working, now only in the background.
    await waitFor(() => expect(pill()).toHaveTextContent("Working in the background: 1 task"));
    expect(snap(backend, s).tasks[0]).toMatchObject({ task_id: "t-a", running: true });
  });

  it("each running subagent and task has its own Stop; it shows pending, and the task is ended only when the daemon says so", async () => {
    const user = userEvent.setup();
    const backend = backgroundBackend();
    const { s } = await open(backend);
    const h = hold(backend, "session.command");
    // The card in the transcript and the row in the sidebar both stop it.
    const side = () => within(screen.getByTestId("side-subagents"));
    expect(screen.getAllByRole("button", { name: "Stop subagent Audit the billing routes" })).toHaveLength(2);
    await user.click(await side().findByRole("button", { name: "Stop subagent Audit the billing routes" }));
    await waitFor(() => screen.getAllByRole("button", { name: "Stop subagent Audit the billing routes" }).forEach((b) => expect(b).toHaveAttribute("aria-busy", "true")));
    expect(snap(backend, s).subagents.find((x) => x.call_id === "c-bill")?.running).toBe(true); // nothing is shown that the daemon has not confirmed
    h.release();
    await waitFor(() => expect(snap(backend, s).subagents.find((x) => x.call_id === "c-bill")?.running).toBe(false));
    expect(commands(backend)).toEqual([{ command: "stop_task", task_id: "task-bill" }]);
    // Finished work is listed newest first: the one just stopped leads.
    await waitFor(() => expect(within(screen.getByTestId("side-subagents")).getAllByTestId("side-subagent")[0]).toHaveTextContent("Stopped"));
    // The test run still goes: the session is working, with one task.
    expect(pill()).toHaveTextContent("Working in the background: 1 task");
    // A task that is not a subagent has its own.
    await user.click(screen.getByRole("button", { name: "Stop task pnpm test --run" }));
    await waitFor(() => expect(h.pending()).toBe(1));
    h.release();
    await waitFor(() => expect(snap(backend, s).phase.state).toBe("idle"));
    expect(commands(backend).map((c) => c.task_id)).toEqual(["task-bill", "task-suite"]);
    expect(screen.queryByTestId("side-tasks")).toBeNull();
  });

  it("a Stop the daemon refuses is said, and the task keeps running", async () => {
    const user = userEvent.setup();
    const backend = backgroundBackend();
    const { s } = await open(backend);
    const h = hold(backend, "session.command");
    h.failAll({ code: "unsupported", message: "not supported: stop_task", data: null });
    await user.click(await screen.findByRole("button", { name: "Stop task pnpm test --run" }));
    expect(await screen.findByText("Could not stop the task")).toBeInTheDocument();
    expect(snap(backend, s).background_running).toBe(2);
    await waitFor(() => expect(screen.getByRole("button", { name: "Stop task pnpm test --run" })).toBeEnabled());
  });

  it("an agent that cannot stop a task has no Stop buttons", async () => {
    const backend = backgroundBackend();
    const s = sessionByName(backend, NAME);
    backend.patchSession(s.host, s.id, { capabilities: { ...s.snap.capabilities, stop_task: false } });
    await open(backend);
    await screen.findByTestId("side-subagents");
    expect(screen.queryByRole("button", { name: /^Stop / })).toBeNull();
    // Interrupt is still the session's.
    expect(screen.getByRole("button", { name: "Interrupt" })).toBeEnabled();
  });
});

describe("the session timeline in Developer mode", () => {
  it("words the background work and the turn the agent started by itself, with the cause of each phase", async () => {
    const backend = backgroundBackend();
    backend.hub.settings["developer.mode"] = true;
    const s = sessionByName(backend, NAME);
    await renderApp(`/session/${s.host}/${s.id}?tab=diagnostics`, backend);
    await screen.findByTestId("session-diagnostics");
    const text = () => document.querySelector('[data-testid="session-diagnostics"]')?.textContent ?? "";
    // What the scripted history holds.
    await waitFor(() => expect(text()).toContain("A background task started — task-bill (local_agent), spawned by call c-bill"));
    expect(text()).toContain("Task task-inv ended successfully (completed), spawned by call c-inv");
    expect(text()).toContain("The agent started a turn on its own");
    expect(text()).toContain("run on the agent's own initiative (task-notification)");
    expect(text()).toContain("The agent lists 2 background tasks as running");
    expect(text()).toMatch(/Tool Read started — call i-r1, made by the subagent of call c-inv/);
    // And what happens while it is open: the phase says why it changed.
    act(() => {
      work(backend, s).endTask("task-bill", "completed");
      work(backend, s).endTask("task-suite", "completed");
    });
    await waitFor(() => expect(text()).toContain("Idle — background task task-suite ended"));
    act(() => work(backend, s).startOwnTurn());
    await waitFor(() => expect(text()).toContain("Working — the agent started a turn on its own"));
    act(() => work(backend, s).startTask({ task_id: "t-z", kind: "local_bash", description: "x", call_id: "c-z" }));
    // The turn ends while a task runs: the session is still Working, so there is no phase to word; the task's end is what makes it Idle.
    act(() => work(backend, s).finishOwnTurn("ok"));
    await waitFor(() => expect(text()).toMatch(/The turn ended: 1 turn[^]*, a reply of 2 characters/));
    expect(text()).not.toContain("Idle — the turn ended");
    act(() => work(backend, s).endTask("t-z", "completed"));
    await waitFor(() => expect(text()).toContain("Idle — background task t-z ended"));
  });
});
