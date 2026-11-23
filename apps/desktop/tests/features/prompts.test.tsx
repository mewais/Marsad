import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import type { HistoryPage, LaunchReply, SessionEvent } from "../../src/bridge/wire";
import { quietBackend, renderApp, sessionByName } from "../helpers";

type Ref = ReturnType<typeof sessionByName>;
const send = (b: FixtureBackend, s: Ref, text: string) => b.request("session.command", { id: s.id, command: { command: "prompt", text } }, { host: s.host });
const history = (b: FixtureBackend, s: Ref) => b.request<HistoryPage>("session.history", { id: s.id, limit: 1000 }, { host: s.host }).then((p) => p.events);
const kinds = (events: SessionEvent[]) => events.map((e) => (e.kind === "agent" ? e.event.type : e.kind));

async function open(name: string, backend = quietBackend()) {
  const s = sessionByName(backend, name);
  const r = await renderApp(`/session/${s.host}/${s.id}`, backend);
  await screen.findByRole("heading", { name });
  return { ...r, s, box: await screen.findByRole("textbox", { name: "Message" }) as HTMLTextAreaElement };
}

describe("fixture backend: the daemon's contract for prompts", () => {
  it("journals the first prompt of a new session as an input event, right after the start", async () => {
    const b = quietBackend();
    const reply = await b.request<LaunchReply>("sessions.launch", { host: "x1", driver: "claude", cwd: "~/src/ledgerline", first_prompt: "Add a health check", resume: { kind: "new" }, labels: {}, workstream: null, overrides: { items: [], env: [], disallowed_tools: [] } });
    const events = (await b.request<HistoryPage>("session.history", { id: reply.id, limit: 100 }, { host: reply.host })).events;
    const k = kinds(events);
    expect(k.slice(0, 3)).toEqual(["spawned", "started", "input"]);
    const input = events[2];
    expect(input).toMatchObject({ kind: "input", command: { command: "prompt", text: "Add a health check" }, status: { status: "sent" } });
    expect(events[1]).toMatchObject({ kind: "started", first_input: (input as { input: number }).input });
  });

  it("refuses a prompt to a session that is not running, as the runtime does, and journals nothing", async () => {
    const b = quietBackend();
    const s = sessionByName(b, "docs-search-index");
    await b.request("sessions.stop", { id: s.id }, { host: s.host });
    const before = (await history(b, s)).length;
    await expect(send(b, s, "hello?")).rejects.toMatchObject({ code: "not_running", message: "the session is not running: the session is exited", data: { error: "not_running", reason: "the session is exited" } });
    expect((await history(b, s)).length).toBe(before);
  });

  it("queues a prompt sent to a busy agent, journals its input_sent when it is taken, and drops it when the session is interrupted or stopped", async () => {
    // The scripted turns are slowed to a standstill, so only the commands move the session.
    const b = quietBackend({ speed: 1e6 });
    const s = sessionByName(b, "csv-schema-worker"); // working
    await send(b, s, "queued one");
    await send(b, s, "queued two");
    let ev = await history(b, s);
    const inputs = ev.filter((e): e is Extract<SessionEvent, { kind: "input" }> => e.kind === "input");
    expect(inputs.slice(-2).map((i) => i.status.status)).toEqual(["queued", "queued"]);
    expect(new Set(inputs.map((i) => i.input)).size).toBe(inputs.length);
    await b.request("session.command", { id: s.id, command: { command: "interrupt" } }, { host: s.host });
    ev = await history(b, s);
    const dropped = ev.filter((e) => e.kind === "agent" && e.event.type === "input_dropped").map((e) => (e as { event: { input: number; reason: string } }).event);
    expect(dropped).toEqual([
      { type: "input_dropped", input: inputs[inputs.length - 2].input, reason: "interrupted" },
      { type: "input_dropped", input: inputs[inputs.length - 1].input, reason: "interrupted" },
    ]);
    // After the interrupt the agent is idle: the next prompt runs at once and nothing is left to drop.
    await send(b, s, "third");
    await send(b, s, "fourth");
    await b.request("sessions.stop", { id: s.id }, { host: s.host });
    ev = await history(b, s);
    expect(ev.filter((e) => e.kind === "agent" && e.event.type === "input_dropped").map((e) => (e as { event: { reason: string } }).event.reason)).toEqual(["interrupted", "interrupted", "shutdown"]);
  });

  it("an agent whose CLI holds the prompt itself journals it sent, acknowledges it when it runs it, and can still lose it to an interrupt", async () => {
    const b = quietBackend({ speed: 1e6 });
    const s = sessionByName(b, "upgrade-tauri-2-5"); // claude, working
    expect(s.snap.capabilities.prompt_while_busy).toBe("queued_by_cli");
    await expect(send(b, s, "after this")).resolves.toEqual({ outcome: "sent" });
    let ev = await history(b, s);
    const input = ev.filter((e): e is Extract<SessionEvent, { kind: "input" }> => e.kind === "input").pop()!;
    expect(input.status).toEqual({ status: "sent" });
    await b.request("session.command", { id: s.id, command: { command: "interrupt" } }, { host: s.host });
    ev = await history(b, s);
    expect(ev.filter((e) => e.kind === "agent" && e.event.type === "input_dropped")).toMatchObject([{ event: { input: input.input, reason: "interrupted" } }]);
  });

  it("takes a queued prompt out of the queue when the turn ends, and says so with input_sent", async () => {
    const b = quietBackend({ speed: 0.05 }); // slow enough that "B" arrives while "A" runs
    const s = sessionByName(b, "docs-search-index"); // idle
    const turns = async () => kinds(await history(b, s)).filter((k) => k === "turn_completed").length;
    const before = await turns();
    await send(b, s, "A");
    await send(b, s, "B");
    await waitFor(async () => expect(await turns()).toBe(before + 2));
    const ev = await history(b, s);
    const tail = kinds(ev);
    expect(tail.indexOf("input_sent")).toBeGreaterThan(tail.indexOf("turn_completed"));
    const bInput = ev.find((e) => e.kind === "input" && e.command.command === "prompt" && e.command.text === "B") as Extract<SessionEvent, { kind: "input" }>;
    expect(bInput.status.status).toBe("queued");
    expect(ev.find((e) => e.kind === "agent" && e.event.type === "input_sent")).toMatchObject({ event: { input: bInput.input } });
  });
});

describe("session view: the first prompt and what became of each prompt", () => {
  it("shows the first prompt typed in the New session dialog as the first message", async () => {
    const user = userEvent.setup();
    const { history: h } = await renderApp();
    await user.keyboard("n");
    const dlg = await screen.findByRole("dialog", { name: "New session" });
    await waitFor(() => expect(within(dlg).getByLabelText("Project folder")).toHaveValue("~/src/ledgerline"));
    await user.type(within(dlg).getByLabelText(/First message/), "Add a health check");
    await user.click(within(dlg).getByRole("button", { name: /Start session/ }));
    await waitFor(() => expect(h.store.get()).toMatch(/^\/session\/x1\//));
    // (The session's name is the first prompt too, so look in the transcript only.)
    await waitFor(() => expect(document.querySelectorAll(".msg-user")).toHaveLength(1));
    const bubble = document.querySelector(".msg-user")!;
    expect(bubble).toHaveTextContent("Add a health check");
    expect(bubble).toHaveAttribute("data-delivery", "sent");
  });

  it("a prompt to a busy agent is shown queued, then not sent once the turn is interrupted", async () => {
    const user = userEvent.setup();
    const { box } = await open("csv-schema-worker");
    await user.type(box, "run the other tests too{Enter}");
    const bubble = (await screen.findByText("run the other tests too")).closest(".msg-user") as HTMLElement;
    await waitFor(() => expect(bubble).toHaveAttribute("data-delivery", "queued"));
    expect(bubble).toHaveTextContent("Queued");
    await user.click(screen.getByRole("button", { name: "Interrupt" }));
    await waitFor(() => expect(bubble).toHaveAttribute("data-delivery", "not_sent"));
    expect(bubble).toHaveTextContent("Not sent — interrupted");
    expect(bubble).not.toHaveTextContent("Queued");
  });

  it("a prompt the CLI holds is shown sent, and not sent if the interrupt cancels it", async () => {
    const user = userEvent.setup();
    const { box } = await open("upgrade-tauri-2-5", quietBackend({ speed: 1e6 }));
    await user.type(box, "then update the docs{Enter}");
    const bubble = (await screen.findByText("then update the docs")).closest(".msg-user") as HTMLElement;
    await waitFor(() => expect(bubble).toHaveAttribute("data-delivery", "sent"));
    expect(bubble).not.toHaveTextContent("Queued");
    await user.click(screen.getByRole("button", { name: "Interrupt" }));
    await waitFor(() => expect(bubble).toHaveTextContent("Not sent — interrupted"));
  });

  it("ending the session says the same of what was still queued", async () => {
    const user = userEvent.setup();
    const { box } = await open("csv-schema-worker");
    await user.type(box, "and after that, lint{Enter}");
    const bubble = (await screen.findByText("and after that, lint")).closest(".msg-user") as HTMLElement;
    await waitFor(() => expect(bubble).toHaveAttribute("data-delivery", "queued"));
    await user.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(bubble).toHaveTextContent("Not sent — session ended"));
  });

  it("a queued prompt loses its label when the agent takes it", async () => {
    const user = userEvent.setup();
    const { box } = await open("docs-search-index");
    await user.type(box, "first{Enter}");
    await screen.findByText("first");
    await user.type(box, "second{Enter}");
    const second = (await screen.findByText("second")).closest(".msg-user") as HTMLElement;
    await waitFor(() => expect(second).toHaveAttribute("data-delivery", "sent"), { timeout: 3000 });
    expect(second).not.toHaveTextContent("Queued");
    expect(await screen.findByText(/Done: second/, undefined, { timeout: 3000 })).toBeInTheDocument();
  });

  it("an input the daemon did not take is not shown as a normal message", async () => {
    const b = quietBackend();
    const s = sessionByName(b, "docs-search-index");
    const { box } = await open("docs-search-index", b);
    // A journaled input that was refused (as a peer would report): the daemon's status decides how it is shown.
    await act(async () => {
      const entry = (b as unknown as { hosts: Map<string, { sessions: Map<string, { events: SessionEvent[]; subs: Set<(e: unknown) => void> }> }> }).hosts.get(s.host)!.sessions.get(s.id)!;
      const last = entry.events[entry.events.length - 1].cursor;
      const ev = { cursor: last + 65536, t: Date.now(), kind: "input", input: 9001, command: { command: "prompt", text: "never ran" }, status: { status: "not_running", reason: "the process is not running" } } as SessionEvent;
      entry.events.push(ev);
      entry.subs.forEach((f) => f(ev));
    });
    const bubble = (await screen.findByText("never ran")).closest(".msg-user") as HTMLElement;
    expect(bubble).toHaveAttribute("data-delivery", "not_sent");
    expect(bubble).toHaveTextContent("Not sent — the session was not running");
    expect(bubble).toHaveTextContent("the process is not running");
    void box;
  });
});

describe("composer: Up and Down recall earlier prompts", () => {
  async function primed(count: number) {
    const b = quietBackend();
    const s = sessionByName(b, "docs-search-index");
    const start = s.snap.turns_completed;
    for (let i = 1; i <= count; i++) await send(b, s, `q${i}`);
    await waitFor(() => expect(b.snapshot(s.host, s.id).turns_completed).toBe(start + count), { timeout: 5000 });
    return open("docs-search-index", b);
  }


  it("Up recalls the newest prompt, then older ones; Down comes back and restores the unsent draft", async () => {
    const user = userEvent.setup();
    const { box } = await primed(3);
    await user.type(box, "my draft");
    await user.keyboard("{Home}{ArrowUp}");
    expect(box).toHaveValue("q3");
    await user.keyboard("{ArrowUp}");
    expect(box).toHaveValue("q2");
    await user.keyboard("{ArrowDown}");
    expect(box).toHaveValue("q3");
    await user.keyboard("{ArrowDown}");
    expect(box).toHaveValue("my draft");
    await user.keyboard("{ArrowDown}");
    expect(box).toHaveValue("my draft");
  });

  it("does nothing while the caret is not at the very start (Up) or very end (Down) of a draft", async () => {
    const user = userEvent.setup();
    const { box } = await primed(2);
    await user.type(box, "line one{Shift>}{Enter}{/Shift}line two");
    await user.keyboard("{ArrowUp}");
    expect(box).toHaveValue("line one\nline two");
    box.setSelectionRange(9, 9); // start of the last line: not the start of the text
    await user.keyboard("{ArrowUp}");
    expect(box).toHaveValue("line one\nline two");
    box.setSelectionRange(0, 3); // at the start, but with a selection
    await user.keyboard("{ArrowUp}");
    expect(box).toHaveValue("line one\nline two");
    box.setSelectionRange(0, 0);
    await user.keyboard("{ArrowUp}");
    expect(box).toHaveValue("q2");
  });

  it("editing a recalled prompt makes it the draft: Down no longer restores the old one", async () => {
    const user = userEvent.setup();
    const { box } = await primed(2);
    await user.type(box, "old draft");
    await user.keyboard("{Home}{ArrowUp}");
    expect(box).toHaveValue("q2");
    await user.keyboard("{End}!");
    expect(box).toHaveValue("q2!");
    await user.keyboard("{ArrowDown}"); // at the end of a draft that is no longer a recall: nothing
    expect(box).toHaveValue("q2!");
    await user.keyboard("{Home}{ArrowUp}");
    expect(box).toHaveValue("q2");
    await user.keyboard("{ArrowDown}");
    expect(box).toHaveValue("q2!");
  });

  it("the slash menu stays closed while walking through the history, and Enter sends the recalled text", async () => {
    const user = userEvent.setup();
    const b = quietBackend();
    const s = sessionByName(b, "docs-search-index");
    await send(b, s, "/zzz");
    await waitFor(() => expect(b.snapshot(s.host, s.id).turns_completed).toBe(s.snap.turns_completed + 1));
    const { box } = await open("docs-search-index", b);
    await user.click(box);
    await user.keyboard("{ArrowUp}");
    expect(box).toHaveValue("/zzz");
    expect(screen.queryByRole("listbox", { name: "Slash commands" })).toBeNull();
    // typing a command still opens it
    await user.clear(box);
    await user.type(box, "/");
    expect(screen.getByRole("listbox", { name: "Slash commands" })).toBeInTheDocument();
  });

  it("pages older prompts in when the user walks past the oldest loaded one, without blocking typing", async () => {
    const user = userEvent.setup();
    const { box, backend } = await primed(14);
    await user.click(box);
    const earlier = () => backend.requests.filter((r) => r.method === "session.history" && (r.params as { before?: number }).before !== undefined).length;
    for (let i = 14; i >= 1; i--) {
      await user.keyboard("{ArrowUp}");
      await waitFor(() => expect(box).toHaveValue(`q${i}`), { timeout: 3000 });
      // The initial page holds fewer than 14 prompts, so the walk had to ask for older events.
      if (i === 14) expect(earlier()).toBe(0);
    }
    expect(earlier()).toBeGreaterThan(0);
    // Past the oldest prompt of the whole session there is nothing more: the box keeps the oldest one.
    for (let i = 0; i < 40; i++) {
      await user.keyboard("{ArrowUp}");
      if (screen.queryByText("Loading earlier prompts…") === null && (box as HTMLTextAreaElement).value === "Work on docs-search-index in docs-site (main).") break;
    }
    await waitFor(() => expect(box).toHaveValue("Work on docs-search-index in docs-site (main)."));
    expect(screen.queryByText("Loading earlier prompts…")).toBeNull();
  });

  it("typing stays possible while older prompts are being fetched, and an edit cancels the walk", async () => {
    const user = userEvent.setup();
    const { box, backend } = await primed(14);
    // The real signal that a background load is over: the request for the older page settling.
    const olderPages: Promise<unknown>[] = [];
    const request = backend.request.bind(backend);
    backend.request = (<T,>(method: string, params?: unknown, opts?: object) => {
      const r = request<T>(method, params, opts);
      if (method === "session.history" && (params as { before?: number }).before !== undefined) olderPages.push(r.then(() => undefined, () => undefined));
      return r;
    }) as typeof backend.request;
    await user.click(box);
    for (let i = 14; i >= 1 && !olderPages.length; i--) await user.keyboard("{ArrowUp}");
    expect(olderPages.length).toBeGreaterThan(0);
    // Whatever page the walk is on, typing now makes the text the draft and ends the walk.
    await user.keyboard("{End} ok");
    expect(box.value.endsWith(" ok")).toBe(true);
    const typed = box.value;
    // Let the cancelled load finish (its reply arrives and React settles), then check it did not touch the box.
    await act(async () => {
      await Promise.all(olderPages);
    });
    expect(box.value).toBe(typed);
    expect(screen.queryByText("Loading earlier prompts…")).toBeNull();
  });
});
