import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { BACKGROUND_SPEC } from "../../src/bridge/fixture/background";
import type { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import { SPECS } from "../../src/bridge/fixture/sessions";
import type { SubagentInfo, SubagentSupport, TaskOutcome } from "../../src/bridge/wire";
import { hold, quietBackend, renderApp, sessionByName } from "../helpers";

/** The fixture's background scenario (a Claude session): a billing subagent still running, an invoice subagent that finished. */
const backgroundBackend = () => quietBackend({ specs: [...SPECS, BACKGROUND_SPEC] });
const NAME = "route-auth-audit";
const NONE: SubagentSupport = { transcript: false, live_state: false, usage: false, steer: false, background: false };

type Ref = ReturnType<typeof sessionByName>;

/** Give the session the subagents and the capabilities the test needs (before it is opened, as the other fixtures do). */
function prepare(backend: FixtureBackend, subagents?: SubagentInfo[], subagentCaps?: Partial<SubagentSupport>): Ref {
  const s = sessionByName(backend, NAME);
  const caps = { ...s.snap.capabilities, subagents: { ...NONE, ...subagentCaps } };
  backend.patchSession(s.host, s.id, { capabilities: caps, ...(subagents ? { subagents } : {}) });
  return s;
}

async function open(backend: FixtureBackend, s: Ref, path?: string, name = NAME) {
  const r = await renderApp(path ?? `/session/${s.host}/${s.id}`, backend);
  await screen.findByRole("heading", { name });
  return r;
}

const sub = (call_id: string, description: string, o: Partial<SubagentInfo> = {}): SubagentInfo => ({
  call_id, name: "Agent", subagent_type: "Explore", description, started_at_ms: Date.now() - 65_000, running: false, tool_calls: 0, ...o,
});
const ended = (call_id: string, description: string, outcome: TaskOutcome | null, o: Partial<SubagentInfo> = {}) => sub(call_id, description, { running: false, outcome, ended_at_ms: Date.now() - 1000, ...o });
const commands = (b: FixtureBackend) => b.requests.filter((r) => r.method === "session.command").map((r) => (r.params as { command: Record<string, unknown> }).command);

/** The strip's rows, All filter, with the list open. */
async function strip(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByTestId("subagent-strip-toggle"));
  await user.click(within(screen.getByTestId("subagent-strip")).getByRole("button", { name: "All" }));
  return within(screen.getByTestId("subagent-strip-list")).getAllByTestId("subagent-strip-row");
}
const rowFor = (rows: HTMLElement[], description: string) => rows.find((r) => r.textContent?.includes(description)) as HTMLElement;

describe("a subagent's status word is its outcome, or Working while it runs", () => {
  it("each outcome reads as its own label, and Lost and Unknown say what the daemon could not know", async () => {
    const user = userEvent.setup();
    const backend = backgroundBackend();
    const s = prepare(backend, [
      sub("c-run", "Still going", { running: true }),
      ended("c-ok", "Shipped", "completed", { ok: true }),
      ended("c-bad", "Broke", "failed", { ok: false }),
      ended("c-halt", "Halted", "stopped", { ok: false }),
      ended("c-lost", "Process died", "lost", { ok: false }),
      ended("c-unk", "No report", null),
    ]);
    await open(backend, s);
    const rows = await strip(user);
    expect(rows).toHaveLength(6);
    expect(within(rowFor(rows, "Still going")).getByText("Working")).toBeInTheDocument();
    expect(within(rowFor(rows, "Shipped")).getByText("Finished")).toBeInTheDocument();
    expect(within(rowFor(rows, "Broke")).getByText("Failed")).toBeInTheDocument();
    expect(within(rowFor(rows, "Halted")).getByText("Stopped")).toBeInTheDocument();
    const lost = within(rowFor(rows, "Process died")).getByText("Lost");
    expect(lost).toHaveAttribute("title", "The agent process ended while it ran.");
    const unknown = within(rowFor(rows, "No report")).getByText("Unknown");
    expect(unknown).toHaveAttribute("title", "State not available.");
    // A working subagent is never shown as ended, and an ended one never as working.
    expect(rowFor(rows, "Still going")).toHaveAttribute("data-running");
    expect(rowFor(rows, "Shipped")).not.toHaveAttribute("data-running");
  });

  it("the transcript card of an ended subagent carries the same label", async () => {
    const backend = backgroundBackend();
    // The billing subagent's card in the transcript: its process ended while it ran.
    const s = prepare(backend, [ended("c-bill", "Audit the billing routes", "lost", { ok: false, task_id: "task-bill" })]);
    await open(backend, s);
    const cards = await screen.findAllByTestId("subagent");
    expect(within(cards[0]).getByText("Lost")).toBeInTheDocument();
  });
});

describe("a subagent's tool calls, tokens and cost are shown only when the driver reports them", () => {
  const withUsage = (o: Partial<SubagentInfo> = {}) => sub("c-u", "Counted", { running: false, outcome: "completed", ok: true, tool_calls: 3, usage: { total_tokens: 12_400, tool_uses: 3, duration_ms: 1000, cost_usd: 0.42 }, ...o });

  it("nothing is shown for an agent that reports none of it", async () => {
    const user = userEvent.setup();
    const backend = backgroundBackend();
    const s = prepare(backend, [withUsage()]);
    await open(backend, s);
    const row = rowFor(await strip(user), "Counted");
    expect(row).not.toHaveTextContent(/tool call/);
    expect(row).not.toHaveTextContent(/tokens/);
    expect(row).not.toHaveTextContent(/\$/);
  });

  it("tool calls need the live state, tokens and cost need usage, each shown when present", async () => {
    const user = userEvent.setup();
    const backend = backgroundBackend();
    const s = prepare(backend, [withUsage()], { live_state: true, usage: true });
    await open(backend, s);
    const row = rowFor(await strip(user), "Counted");
    expect(row).toHaveTextContent("3 tool calls");
    expect(row).toHaveTextContent("12k tokens");
    expect(row).toHaveTextContent("$0.42");
  });

  it("usage the agent did not report is not shown as zero; the tool calls it did report still are", async () => {
    const user = userEvent.setup();
    const backend = backgroundBackend();
    const s = prepare(backend, [withUsage({ usage: null, tool_calls: 0 })], { live_state: true, usage: true });
    await open(backend, s);
    const row = rowFor(await strip(user), "Counted");
    expect(row).toHaveTextContent("0 tool calls");
    expect(row).not.toHaveTextContent(/tokens/);
    expect(row).not.toHaveTextContent(/\$/);
  });

  it("the cost is hidden on its own when the agent reports tokens but no cost", async () => {
    const user = userEvent.setup();
    const backend = backgroundBackend();
    const s = prepare(backend, [withUsage({ usage: { total_tokens: 900, tool_uses: 1, duration_ms: 1, cost_usd: null } })], { usage: true });
    await open(backend, s);
    const row = rowFor(await strip(user), "Counted");
    expect(row).toHaveTextContent("900 tokens");
    expect(row).not.toHaveTextContent(/\$/);
    expect(row).not.toHaveTextContent(/tool call/);
  });
});

describe("steering a running subagent", () => {
  const RUNNING_TASK = "task-bill"; // the billing subagent (c-bill) is the running one of the scenario

  it("a running subagent of a driver that can steer takes a message, sent to its task on the parent session", async () => {
    const user = userEvent.setup();
    const backend = backgroundBackend();
    const s = prepare(backend, undefined, { steer: true });
    await open(backend, s, `/session/${s.host}/${s.id}?sub=c-bill`);
    const form = await screen.findByTestId("subagent-steer");
    const box = within(form).getByRole("textbox", { name: "Message this sub-agent" });
    expect(box).toHaveAttribute("placeholder", "Message this sub-agent…");
    expect(form).toHaveTextContent("It reads your message at its next step.");
    // Read only is not said here: the box is the way to talk to it.
    expect(screen.queryByTestId("subagent-readonly")).toBeNull();
    const send = within(form).getByRole("button", { name: "Send" });
    expect(send).toBeDisabled();
    await user.type(box, "Check the webhook signature path too");
    await user.click(send);
    await waitFor(() => expect(commands(backend)).toEqual([{ command: "steer_subagent", task_id: RUNNING_TASK, text: "Check the webhook signature path too" }]));
    await waitFor(() => expect(box).toHaveValue(""));
    // Stop stays beside it.
    expect(screen.getByRole("button", { name: "Stop subagent Audit the billing routes" })).toBeInTheDocument();
  });

  it("the box is not shown for a driver that cannot steer, and the transcript is read only as before", async () => {
    const backend = backgroundBackend();
    const s = prepare(backend, undefined, { steer: false });
    await open(backend, s, `/session/${s.host}/${s.id}?sub=c-bill`);
    expect(await screen.findByTestId("subagent-readonly")).toHaveTextContent("It is read only");
    expect(screen.queryByTestId("subagent-steer")).toBeNull();
  });

  it("a subagent that has finished takes no message, even from a driver that can steer", async () => {
    const backend = backgroundBackend();
    const s = prepare(backend, undefined, { steer: true });
    await open(backend, s, `/session/${s.host}/${s.id}?sub=c-inv`);
    expect(await screen.findByTestId("subagent-readonly")).toBeInTheDocument();
    expect(screen.queryByTestId("subagent-steer")).toBeNull();
  });

  it("a message the daemon refuses is kept in the box, so nothing typed is lost", async () => {
    const user = userEvent.setup();
    const backend = backgroundBackend();
    const s = prepare(backend, undefined, { steer: true });
    await open(backend, s, `/session/${s.host}/${s.id}?sub=c-bill`);
    const h = hold(backend, "session.command");
    h.failAll({ code: "rejected", message: "not running", data: null });
    const box = await screen.findByRole("textbox", { name: "Message this sub-agent" });
    await user.type(box, "Also cover refunds");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByText("Could not message the subagent")).toBeInTheDocument();
    expect(box).toHaveValue("Also cover refunds");
  });
});

describe("the Interrupt tooltip says what it leaves running", () => {
  it("with a turn open and running background subagents, Interrupt stops only the main agent", async () => {
    const backend = quietBackend();
    const s = sessionByName(backend, "csv-schema-worker"); // working: a turn is open
    backend.patchSession(s.host, s.id, { capabilities: { ...s.snap.capabilities, subagents: { ...NONE, background: true } }, subagents: [sub("c-run", "Still going", { running: true })] });
    await open(backend, s, `/session/${s.host}/${s.id}`, "csv-schema-worker");
    expect(screen.getByRole("button", { name: "Interrupt" })).toHaveAttribute("title", "Stops the main agent; background sub-agents keep running");
  });

  it("with no running subagent the tooltip says what it did before", async () => {
    const backend = backgroundBackend();
    const s = prepare(backend, [ended("c-x", "Done", "completed", { ok: true })], { background: true });
    await open(backend, s);
    expect(screen.getByRole("button", { name: "Interrupt" })).toHaveAttribute("title", "No turn is running. Stop the 2 background tasks; the agent stays open.");
  });

  it("an agent that does not keep its background subagents on interrupt keeps the tooltip it always had", async () => {
    const backend = backgroundBackend();
    const s = prepare(backend, undefined, { background: false });
    await open(backend, s);
    expect(screen.getByRole("button", { name: "Interrupt" })).toHaveAttribute("title", "No turn is running. Stop the 2 background tasks; the agent stays open.");
  });
});

describe("ending a session that owns running subagents asks first", () => {
  it("names how many it would stop; Cancel sends nothing, End session sends the stop", async () => {
    const user = userEvent.setup();
    const backend = backgroundBackend();
    const s = prepare(backend, undefined, { background: true });
    await open(backend, s);
    await user.click(screen.getByRole("button", { name: "End session" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("This also stops 1 running sub-agent.");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(backend.requests.filter((r) => r.method === "sessions.stop")).toHaveLength(0);
    await user.click(screen.getByRole("button", { name: "End session" }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "End session" }));
    await waitFor(() => expect(backend.requests.filter((r) => r.method === "sessions.stop")).toHaveLength(1));
  });

  it("counts every running subagent", async () => {
    const user = userEvent.setup();
    const backend = backgroundBackend();
    const s = prepare(backend, [sub("c-a", "First", { running: true }), sub("c-b", "Second", { running: true }), ended("c-c", "Done", "completed", { ok: true })], { background: true });
    await open(backend, s);
    await user.click(screen.getByRole("button", { name: "End session" }));
    expect(await screen.findByRole("dialog")).toHaveTextContent("This also stops 2 running sub-agents.");
  });

  it("an agent without background subagents ends at once, as it always did", async () => {
    const user = userEvent.setup();
    const backend = backgroundBackend();
    const s = prepare(backend, undefined, { background: false });
    await open(backend, s);
    await user.click(screen.getByRole("button", { name: "End session" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    await waitFor(() => expect(backend.requests.filter((r) => r.method === "sessions.stop")).toHaveLength(1));
  });
});
