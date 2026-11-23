import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import { MAX_DIRECTORY_TERMINALS } from "../../src/bridge/fixture/pty";
import type { PtyEvent, PtyStarted } from "../../src/bridge/wire";
import { hold, quietBackend, renderApp, sessionByName } from "../helpers";

/** xterm.js stands in (it needs a real canvas): every terminal the tab creates is recorded, with what it was shown and whether it was disposed. */
const xt = vi.hoisted(() => {
  class FakeTerm {
    cols = 100;
    rows = 30;
    disposed = false;
    shown = "";
    focused = 0;
    helper: HTMLTextAreaElement;
    private handlers = new Set<(d: string) => void>();
    constructor(el: HTMLElement) {
      this.helper = document.createElement("textarea");
      el.append(this.helper);
    }
    write(b: Uint8Array) {
      this.shown += new TextDecoder().decode(b);
    }
    onData(cb: (d: string) => void) {
      this.handlers.add(cb);
      return { dispose: () => void this.handlers.delete(cb) };
    }
    /** What the user typing into the terminal gives xterm.js to send. */
    type(d: string) {
      this.handlers.forEach((h) => h(d));
    }
    focus() {
      this.focused++;
      this.helper.focus();
    }
    dispose() {
      this.disposed = true;
      this.helper.remove();
    }
  }
  return { terms: [] as FakeTerm[], FakeTerm };
});
vi.mock("../../src/features/session/tabs/xterm", () => ({
  createTerminal: (el: HTMLElement) => {
    const term = new xt.FakeTerm(el);
    xt.terms.push(term);
    return { term, fit: { fit: () => undefined } };
  },
}));

beforeAll(() => {
  // jsdom has none; the tab only needs it not to throw.
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
beforeEach(() => {
  xt.terms.length = 0;
});

/** The session page of a local, idle session (`x1`), on its Terminal tab. */
async function openTerminal(backend: FixtureBackend = quietBackend()) {
  const s = sessionByName(backend, "docs-search-index");
  const r = await renderApp(`/session/${s.host}/${s.id}`, backend);
  await screen.findByRole("heading", { name: "docs-search-index" });
  const user = userEvent.setup();
  return { ...r, s, user, term: () => openTab(user, "Terminal") };
}

const openTab = (user: ReturnType<typeof userEvent.setup>, name: string) => user.click(screen.getByRole("tab", { name }));
const requests = (b: FixtureBackend, method: string) => b.requests.filter((r) => r.method === method);
const ptySubs = (b: FixtureBackend) => b.subscriptions.filter((s) => s.topic.startsWith("pty.output:"));
const last = () => xt.terms[xt.terms.length - 1];
/** The line above the terminal: what the tab knows, in words. */
const status = () => document.querySelector(".term-status") as HTMLElement;
const prompted = (t: { shown: string }) => waitFor(() => expect(t.shown).toContain("$ "));
const typed = async (t: { type: (d: string) => void; shown: string }, line: string, expectShown: string) => {
  act(() => t.type(`${line}\r`));
  await waitFor(() => expect(t.shown).toContain(expectShown));
};

describe("Terminal tab: attach to the session's terminal, never start another", () => {
  it("shows what is pending, then the shell; the first visit starts one shell and tells the host the size once attached", async () => {
    const backend = quietBackend();
    const { s, term } = await openTerminal(backend);
    const h = hold(backend, "pty.list");
    await term();
    expect(status()).toHaveTextContent("Looking for this session's terminal on the host…");
    expect(requests(backend, "pty.start")).toHaveLength(0); // nothing is started before the host has said there is none
    const g = hold(backend, "pty.start");
    h.release();
    await waitFor(() => expect(status()).toHaveTextContent("Starting a shell on the host…"));
    g.release();
    await waitFor(() => expect(status()).toHaveTextContent(s.snap.cwd!));
    expect(screen.getByRole("button", { name: "Close terminal" })).toBeEnabled();
    expect(requests(backend, "pty.start")).toHaveLength(1);
    expect(requests(backend, "pty.start")[0].params).toMatchObject({ target: { session: s.id } });
    await prompted(last());
    expect(requests(backend, "pty.resize").map((r) => r.params)).toEqual([{ id: backend.ptys.list(s.host)[0].id, cols: 100, rows: 30 }]);
    // And it is the user's shell: what is typed reaches it, in order, and its output comes back.
    await typed(last(), "echo hello-42", "hello-42");
    expect(backend.ptys.inputOf(s.host, s.id).join("")).toBe("echo hello-42\r");
    expect(last().focused).toBeGreaterThan(0);
  });

  it("coming back to the tab attaches to the same terminal with its scrollback: no second shell, and what ran meanwhile is there", async () => {
    const backend = quietBackend();
    const { s, term, user } = await openTerminal(backend);
    await term();
    await prompted(last());
    await typed(last(), "echo first-visit", "first-visit");
    const id = backend.ptys.list(s.host)[0].id;
    await openTab(user, "Activity");
    expect(last().disposed).toBe(true);
    // The shell goes on while the tab is away.
    await act(async () => void (await backend.request("pty.input", { id, data: "echo while-away\r" }, { host: s.host })));
    await term();
    await waitFor(() => expect(last().shown).toContain("while-away"));
    expect(last().shown).toContain("first-visit");
    expect(requests(backend, "pty.start")).toHaveLength(1);
    expect(backend.ptys.list(s.host).map((p) => p.id)).toEqual([id]);
    // It is live again, and typing works: the same shell answers.
    await typed(last(), "pwd", s.snap.cwd!);
  });

  it("opening and leaving the tab many times starts one shell and leaves no subscription and no xterm behind", async () => {
    const backend = quietBackend();
    const { s, term, user } = await openTerminal(backend);
    const N = 6;
    for (let i = 0; i < N; i++) {
      await term();
      await waitFor(() => expect(ptySubs(backend)).toHaveLength(1));
      await prompted(last());
      expect(xt.terms.filter((t) => !t.disposed)).toHaveLength(1);
      await openTab(user, "Activity");
      expect(ptySubs(backend)).toHaveLength(0);
      expect(backend.ptys.subscribers).toBe(0);
      expect(xt.terms.every((t) => t.disposed)).toBe(true);
    }
    expect(xt.terms).toHaveLength(N);
    expect(requests(backend, "pty.start")).toHaveLength(1);
    expect(requests(backend, "pty.list")).toHaveLength(N);
    expect(requests(backend, "pty.close")).toHaveLength(0); // leaving only detaches
    expect(backend.ptys.list(s.host)).toHaveLength(1);
  });

  it("leaving the tab while the shell is being started does not close it: it is the session's terminal, and the next visit attaches", async () => {
    const backend = quietBackend();
    const { s, term, user } = await openTerminal(backend);
    const g = hold(backend, "pty.start");
    await term();
    await waitFor(() => expect(g.pending()).toBe(1));
    await openTab(user, "Activity");
    g.release();
    await waitFor(() => expect(backend.ptys.list(s.host)).toHaveLength(1));
    expect(xt.terms).toHaveLength(0); // nothing was drawn for a tab that is gone
    await term();
    await prompted(last());
    expect(requests(backend, "pty.start")).toHaveLength(1);
    expect(requests(backend, "pty.close")).toHaveLength(0);
  });

  it("the session ending leaves its terminal alone", async () => {
    const backend = quietBackend();
    const { s, term, user } = await openTerminal(backend);
    await term();
    await prompted(last());
    await user.click(screen.getByRole("button", { name: "End session" }));
    await screen.findByRole("button", { name: "Continue" });
    expect(requests(backend, "pty.close")).toHaveLength(0);
    expect(backend.ptys.list(s.host)[0].exited).toBe(false);
    await typed(last(), "echo still-mine", "still-mine");
  });
});

describe("Terminal tab: the shell exits, and the terminal is closed", () => {
  it("the shell exiting says so with its status, keeps the output, sends nothing more, and offers a new terminal", async () => {
    const backend = quietBackend();
    const { s, term } = await openTerminal(backend);
    await term();
    await prompted(last());
    act(() => last().type("exit 3\r"));
    await waitFor(() => expect(status()).toHaveTextContent("The shell exited with status 3."));
    expect(last().shown).toContain("exit 3");
    expect(screen.queryByRole("button", { name: "Close terminal" })).toBeNull();
    act(() => last().type("x")); // not confirmed live any more: it goes nowhere
    expect(requests(backend, "pty.input").map((r) => (r.params as { data: string }).data)).not.toContain("x");
    expect(backend.ptys.list(s.host)[0]).toMatchObject({ exited: true, exit_code: 3 });
  });

  it("revisiting an exited terminal shows its exit and output without starting a shell; Open a new terminal replaces it", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { s, term } = await openTerminal(backend);
    await term();
    await prompted(last());
    act(() => last().type("echo bye-1\rexit 7\r"));
    await waitFor(() => expect(status()).toHaveTextContent("exited with status 7"));
    const old = backend.ptys.list(s.host)[0].id;
    await openTab(user, "Activity");
    await term();
    await waitFor(() => expect(status()).toHaveTextContent("The shell exited with status 7."));
    expect(last().shown).toContain("bye-1");
    expect(requests(backend, "pty.start")).toHaveLength(1);
    expect(ptySubs(backend)).toHaveLength(0); // its stream ended with the exit
    const first = last();
    await user.click(screen.getByRole("button", { name: "Open a new terminal" }));
    await waitFor(() => expect(status()).toHaveTextContent(s.snap.cwd!));
    expect(requests(backend, "pty.start")).toHaveLength(2);
    expect(first.disposed).toBe(true);
    const now = backend.ptys.list(s.host);
    expect(now).toHaveLength(1);
    expect(now[0].id).not.toBe(old);
    await typed(last(), "echo again", "again");
  });

  it("Close terminal asks first; cancelling closes nothing; confirming shows the pending state, then 'No terminal is open'", async () => {
    const backend = quietBackend();
    const { s, term, user } = await openTerminal(backend);
    await term();
    await prompted(last());
    await user.click(screen.getByRole("button", { name: "Close terminal" }));
    const dialog = await screen.findByRole("dialog", { name: "Close this terminal?" });
    expect(dialog).toHaveTextContent("The shell and anything running in it will be ended.");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(requests(backend, "pty.close")).toHaveLength(0);
    expect(backend.ptys.list(s.host)[0].exited).toBe(false);

    const h = hold(backend, "pty.close");
    await user.click(screen.getByRole("button", { name: "Close terminal" }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Close terminal" }));
    await waitFor(() => expect(status()).toHaveTextContent("Closing the terminal…"));
    expect(screen.getByRole("button", { name: "Close terminal" })).toHaveAttribute("aria-busy", "true");
    act(() => last().type("x")); // the host has not answered: nothing more goes to the shell
    expect(requests(backend, "pty.input").map((r) => (r.params as { data: string }).data)).not.toContain("x");
    h.release();
    expect(await screen.findByText("No terminal is open")).toBeInTheDocument();
    expect(requests(backend, "pty.close")).toHaveLength(1);
    expect(last().disposed).toBe(true);
    expect(ptySubs(backend)).toHaveLength(0);
    expect(backend.ptys.list(s.host)).toHaveLength(0);

    await user.click(screen.getByRole("button", { name: "Open a new terminal" }));
    await waitFor(() => expect(status()).toHaveTextContent(s.snap.cwd!));
    expect(requests(backend, "pty.start")).toHaveLength(2);
    await prompted(last());
  });

  it("a terminal closed from elsewhere is reported as lost, and Reattach asks the host what is left", async () => {
    const backend = quietBackend();
    const { s, term, user } = await openTerminal(backend);
    await term();
    await prompted(last());
    const id = backend.ptys.list(s.host)[0].id;
    // The host ends the output stream without the shell having exited on its own (here: the daemon dropped it).
    act(() => backend.endSubscriptions(`pty.output:${id}`, { code: "lagged", message: "the stream was dropped" }));
    await waitFor(() => expect(status()).toHaveTextContent("Lost the terminal: the stream was dropped"));
    await user.click(screen.getByRole("button", { name: "Reattach" }));
    await waitFor(() => expect(status()).toHaveTextContent(s.snap.cwd!));
    expect(requests(backend, "pty.start")).toHaveLength(1);
    expect(ptySubs(backend)).toHaveLength(1);
  });
});

describe("Terminal tab: failures say why", () => {
  it("the host cannot be asked: the reason, and Try again attaches once it can", async () => {
    const backend = quietBackend();
    const { s, term, user } = await openTerminal(backend);
    const h = hold(backend, "pty.list");
    h.failAll({ code: "upstream", message: "thinkpad-x1 is not connected" });
    await term();
    expect(await screen.findByText("Terminal not available")).toBeInTheDocument();
    expect(screen.getByText(/thinkpad-x1 is not connected/)).toBeInTheDocument();
    expect(requests(backend, "pty.start")).toHaveLength(0);
    h.healAll();
    await user.click(screen.getByRole("button", { name: "Try again" }));
    h.release();
    await waitFor(() => expect(status()).toHaveTextContent(s.snap.cwd!));
  });

  it("the host refuses to start a shell: the reason, and nothing is attached", async () => {
    const backend = quietBackend();
    const { term } = await openTerminal(backend);
    hold(backend, "pty.start").failAll({ code: "io", message: "io error: spawn /bin/zsh: No such file or directory", data: { marsad_error: "io", message: "spawn /bin/zsh: No such file or directory" } });
    await term();
    expect(await screen.findByText("Terminal not available")).toBeInTheDocument();
    expect(screen.getByText(/spawn \/bin\/zsh/)).toBeInTheDocument();
    expect(xt.terms).toHaveLength(0);
    expect(ptySubs(backend)).toHaveLength(0);
  });

  it("a close the host refuses is reported, and the tab asks the host what became of the terminal", async () => {
    const backend = quietBackend();
    const { s, term, user } = await openTerminal(backend);
    await term();
    await prompted(last());
    hold(backend, "pty.close").failAll({ code: "timeout", message: "timed out: terminal did not end" });
    await user.click(screen.getByRole("button", { name: "Close terminal" }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Close terminal" }));
    expect(await screen.findByText("Could not close the terminal")).toBeInTheDocument();
    // The terminal is still there, and the tab says so from the host's answer, not its own guess.
    await waitFor(() => expect(status()).toHaveTextContent(s.snap.cwd!));
    expect(screen.queryByText("No terminal is open")).toBeNull();
    expect(backend.ptys.list(s.host)).toHaveLength(1);
    expect(requests(backend, "pty.start")).toHaveLength(1);
    await typed(last(), "echo alive", "alive");
  });

  it("an unreachable host: the reason is shown, typing is off and nothing is sent, and it works again when the host is back", async () => {
    const backend = quietBackend();
    const { s, term } = await openTerminal(backend);
    await term();
    await prompted(last());
    const t = last();
    // The local daemon's own link going away is the one every host request depends on.
    act(() => backend.setHubLink({ state: "reconnecting", error: "the daemon went away" }));
    await waitFor(() => expect(status()).toHaveTextContent("Typing is off"));
    expect(screen.getByRole("button", { name: "Close terminal" })).toBeDisabled();
    const sent = requests(backend, "pty.input").length;
    act(() => t.type("echo nope\r"));
    expect(requests(backend, "pty.input")).toHaveLength(sent);
    act(() => backend.setHubLink({ state: "connected" }));
    await waitFor(() => expect(status()).toHaveTextContent(s.snap.cwd!));
    await typed(t, "echo back", "back");
  });
});

describe("Terminal tab: Esc belongs to the shell", () => {
  it("Esc typed in the terminal reaches the shell; the page does not leave, blur or interrupt", async () => {
    const backend = quietBackend();
    const { s, term, user, history } = await openTerminal(backend);
    await term();
    await prompted(last());
    await waitFor(() => expect(document.activeElement).toBe(last().helper));
    const path = history.store.get();
    await user.keyboard("{Escape}");
    // xterm.js turns the key into the byte the shell reads.
    act(() => last().type("\x1b"));
    expect(history.store.get()).toBe(path);
    expect(document.activeElement).toBe(last().helper);
    expect(requests(backend, "session.command")).toHaveLength(0);
    await waitFor(() => expect(backend.ptys.inputOf(s.host).join("")).toContain("\x1b"));
  });

  it("the Close terminal dialog still closes on Esc, and the terminal is untouched", async () => {
    const backend = quietBackend();
    const { s, term, user } = await openTerminal(backend);
    await term();
    await prompted(last());
    await user.click(screen.getByRole("button", { name: "Close terminal" }));
    await screen.findByRole("dialog", { name: "Close this terminal?" });
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(requests(backend, "pty.close")).toHaveLength(0);
    expect(backend.ptys.list(s.host)[0].exited).toBe(false);
  });
});

/** The fixture's terminals follow the host's contract (marsad-agent `host/pty.rs`). */
describe("fixture terminals follow the host's contract", () => {
  const start = (b: FixtureBackend, host: string, target: { session?: string; cwd?: string }) =>
    b.request<PtyStarted>("pty.start", { target, cols: 80, rows: 24 }, { host });
  const subscribe = (b: FixtureBackend, host: string, id: string, from?: number) => {
    const events: PtyEvent[] = [];
    const ends: unknown[] = [];
    const off = b.subscribe(`pty.output:${id}`, { host, from }, (e) => events.push(e as PtyEvent), (e) => ends.push(e));
    return { events, ends, off };
  };
  const tail = <T,>(a: T[]) => a[a.length - 1];
  /** The code of the error a request was refused with. */
  const codeOf = async (p: Promise<unknown>) => ((await p.catch((e) => e)) as { code: string }).code;
  const text = (events: PtyEvent[]) => events.map((e) => (e.kind === "output" ? atob(e.data) : "")).join("");

  it("start is idempotent per session: the live terminal is returned and says it was not created", async () => {
    const b = quietBackend();
    const s = sessionByName(b, "docs-search-index");
    const a = await start(b, s.host, { session: s.id });
    const again = await start(b, s.host, { session: s.id });
    expect([a.created, again.created]).toEqual([true, false]);
    expect(again.pty.id).toBe(a.pty.id);
    expect(((await b.request("pty.list", undefined, { host: s.host })) as { ptys: unknown[] }).ptys).toHaveLength(1);
    expect(await codeOf(start(b, s.host, { session: "nope" }))).toBe("not_found");
  });

  it("output replays from an offset, byte-exact, then follows live", async () => {
    const b = quietBackend();
    const s = sessionByName(b, "docs-search-index");
    const { pty } = await start(b, s.host, { session: s.id });
    await b.request("pty.input", { id: pty.id, data: "echo one\r" }, { host: s.host });
    const whole = subscribe(b, s.host, pty.id);
    await waitFor(() => expect(whole.events).toHaveLength(1));
    const all = text(whole.events);
    expect(whole.events[0]).toMatchObject({ kind: "output", offset: 0 });
    const cut = 12;
    const part = subscribe(b, s.host, pty.id, cut);
    await waitFor(() => expect(part.events).toHaveLength(1));
    expect(part.events[0]).toMatchObject({ kind: "output", offset: cut });
    expect(text(part.events)).toBe(all.slice(cut));
    await b.request("pty.input", { id: pty.id, data: "echo two\r" }, { host: s.host });
    await waitFor(() => expect(text(part.events)).toContain("two"));
    // Offsets are contiguous across the replay and the live output.
    let at = cut;
    for (const e of part.events) {
      if (e.kind !== "output") continue;
      expect(e.offset).toBe(at);
      at += atob(e.data).length;
    }
    whole.off();
    part.off();
  });

  it("an exit ends the stream with `exited`, replayable for a later subscriber; closing and a new open replace it", async () => {
    const b = quietBackend();
    const s = sessionByName(b, "docs-search-index");
    const { pty } = await start(b, s.host, { session: s.id });
    const first = subscribe(b, s.host, pty.id);
    await b.request("pty.input", { id: pty.id, data: "exit 4\r" }, { host: s.host });
    await waitFor(() => expect(first.ends).toEqual([null]));
    expect(tail(first.events)).toEqual({ kind: "exited", code: 4 });
    expect(b.subscriptions.filter((x) => x.topic.startsWith("pty.output:"))).toHaveLength(0);
    // A late subscriber: the scrollback, then the exit, then the end.
    const late = subscribe(b, s.host, pty.id);
    await waitFor(() => expect(late.ends).toEqual([null]));
    expect(late.events[0]).toMatchObject({ kind: "output", offset: 0 });
    expect(tail(late.events)).toEqual({ kind: "exited", code: 4 });
    // One that had it all (the exit included) has nothing left; one from nowhere is refused.
    const total = atob((late.events[0] as { data: string }).data).length;
    expect(total).toBeGreaterThan(0);
    const done = subscribe(b, s.host, pty.id, total + 1);
    const bogus = subscribe(b, s.host, pty.id, total + 2);
    await waitFor(() => expect(done.ends).toEqual([null]));
    expect(done.events).toEqual([]);
    await waitFor(() => expect(bogus.ends).toHaveLength(1));
    expect((bogus.ends[0] as { code: string }).code).toBe("invalid_params");
    // Input to it is refused; the next open replaces it.
    expect(await codeOf(b.request("pty.input", { id: pty.id, data: "x" }, { host: s.host }))).toBe("invalid_params");
    const fresh = await start(b, s.host, { session: s.id });
    expect(fresh.created).toBe(true);
    expect(fresh.pty.id).not.toBe(pty.id);
    expect(((await b.request("pty.list", undefined, { host: s.host })) as { ptys: unknown[] }).ptys).toHaveLength(1);
    // Closing ends it: its subscribers see `exited`.
    const live = subscribe(b, s.host, fresh.pty.id);
    await waitFor(() => expect(live.events).toHaveLength(1));
    await b.request("pty.close", { id: fresh.pty.id }, { host: s.host });
    await waitFor(() => expect(live.ends).toEqual([null]));
    expect(tail(live.events)).toMatchObject({ kind: "exited" });
    expect(((await b.request("pty.list", undefined, { host: s.host })) as { ptys: unknown[] }).ptys).toHaveLength(0);
  });

  it("deleting the session closes its terminal (ending the session does not); directory terminals are capped", async () => {
    const b = quietBackend();
    const s = sessionByName(b, "docs-search-index");
    const { pty } = await start(b, s.host, { session: s.id });
    const watch = subscribe(b, s.host, pty.id);
    await waitFor(() => expect(watch.events).toHaveLength(1));
    await b.request("sessions.stop", { id: s.id }, { host: s.host });
    expect(b.ptys.list(s.host)).toHaveLength(1);
    await b.request("sessions.delete", { id: s.id }, { host: s.host });
    await waitFor(() => expect(watch.ends).toEqual([null]));
    expect(tail(watch.events)).toMatchObject({ kind: "exited" });
    expect(b.ptys.list(s.host)).toHaveLength(0);
    for (let i = 0; i < MAX_DIRECTORY_TERMINALS; i++) expect((await start(b, s.host, { cwd: "/tmp" })).created).toBe(true);
    expect(await codeOf(start(b, s.host, { cwd: "/tmp" }))).toBe("limit_reached");
  });
});
