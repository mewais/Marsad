import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../../src/bridge/wire";
import type { PendingFile } from "../../src/features/session/attachments";
import { appendQuote, clearDraft, onQuote, peekDraft, quoteLines, setDraft } from "../../src/features/session/draft";
import { rawJson, RAW_CAP_BYTES, RawEventDrawer, spanEvents } from "../../src/features/session/RawEventDrawer";
import { TranscriptItem } from "../../src/features/session/TranscriptView";
import { TranscriptCtx } from "../../src/features/session/transcriptContext";
import { groupToolRuns, type RowItem } from "../../src/features/session/toolRuns";
import { createTranscriptFold } from "../../src/features/session/transcript";
import { AppProvider, createApp } from "../../src/state";
import { MAIN } from "../../src/state/scope";
import { quietBackend } from "../helpers";

const KEY = "h1/s1";
const CTX = { key: KEY, host: "h1", id: "s1" };

let n = 0;
const ev = (e: Record<string, unknown>, cursor?: number) => ({ cursor: cursor ?? ++n * 65536, t: 1_000 + n, ...e }) as unknown as SessionEvent;
const agent = (event: Record<string, unknown>, cursor?: number) => ev({ kind: "agent", event }, cursor);
const prompt = (text: string, input = 1) => ev({ kind: "input", input, command: { command: "prompt", text }, status: { status: "sent" } });

/** The rows a transcript shows for these events (tool calls grouped as the transcript groups them). */
const rowsOf = (events: SessionEvent[]): RowItem[] => groupToolRuns(createTranscriptFold(MAIN).update(events));

const bash = (id: string, command: string, result: string) => [
  agent({ type: "tool_call_started", name: "Bash", call_id: id, input: { command }, is_subagent: false }),
  agent({ type: "tool_call_finished", name: "Bash", call_id: id, ok: true, result }),
];

let writeText: ReturnType<typeof vi.fn>;
let app: ReturnType<typeof createApp>;

beforeEach(() => {
  writeText = vi.fn().mockResolvedValue(undefined);
  app = createApp(quietBackend());
});

/** A user for the test, with the clipboard spy installed after it (user-event installs its own stub when it is set up). */
function setup() {
  const user = userEvent.setup();
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  return user;
}

afterEach(() => {
  app.dispose();
  clearDraft(KEY);
  // The composer stand-in the focus test appends is not React's: it is removed here (rendered content is cleaned up by Testing Library).
  document.querySelectorAll('textarea[aria-label="Message"]').forEach((n) => n.remove());
});

/** One row as a transcript draws it: with the session context the quote and raw view need. */
function renderRow(row: RowItem, ctx: typeof CTX | null = CTX) {
  return render(
    <AppProvider value={app}>
      <TranscriptCtx.Provider value={ctx}>
        <TranscriptItem item={row} />
      </TranscriptCtx.Provider>
    </AppProvider>,
  );
}

const menuButton = () => screen.getByRole("button", { name: "Message actions" });
/** Opens the row's menu with the keyboard, as a keyboard user does. */
async function openMenu(user: ReturnType<typeof userEvent.setup>) {
  act(() => menuButton().focus());
  await user.keyboard("{Enter}");
  return screen.getByRole("menu", { name: "Message actions" });
}
async function choose(user: ReturnType<typeof userEvent.setup>, label: string) {
  await openMenu(user);
  await user.click(within(screen.getByRole("menu")).getByRole("menuitem", { name: label }));
}

describe("row menu: keyboard", () => {
  it("is reached from the keyboard: Enter opens it, arrows move through the actions, Enter chooses, Escape closes and returns focus", async () => {
    const user = setup();
    renderRow(rowsOf([prompt("refactor the parser")])[0]);
    await openMenu(user);
    // The first action takes the focus when the menu opens.
    expect(screen.getByRole("menuitem", { name: "Copy as markdown" })).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("menuitem", { name: "Quote in reply" })).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("refactor the parser"));
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("Escape closes the open menu and gives the focus back to its button", async () => {
    const user = setup();
    renderRow(rowsOf([prompt("hello")])[0]);
    await openMenu(user);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(menuButton()).toHaveFocus();
  });

  it("the menu button is a labelled button that says whether the menu is open", async () => {
    const user = setup();
    renderRow(rowsOf([prompt("hello")])[0]);
    expect(menuButton()).toHaveAttribute("aria-haspopup", "menu");
    expect(menuButton()).toHaveAttribute("aria-expanded", "false");
    await openMenu(user);
    expect(menuButton()).toHaveAttribute("aria-expanded", "true");
  });
});

describe("row menu: copy as markdown, per kind", () => {
  it("a prompt copies its text as written", async () => {
    const user = setup();
    renderRow(rowsOf([prompt("please *refactor* the parser")])[0]);
    await choose(user, "Copy as markdown");
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("please *refactor* the parser"));
  });

  it("a response copies its text as written", async () => {
    const user = setup();
    renderRow(rowsOf([agent({ type: "assistant_delta", text: "Done. See `parser.ts`.", message_id: "m" })])[0]);
    await choose(user, "Copy as markdown");
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("Done. See `parser.ts`."));
  });

  it("a tool call copies its name, argument and output as a fenced block", async () => {
    const user = setup();
    renderRow(rowsOf(bash("b1", "pnpm test", "27 passed"))[0]);
    await choose(user, "Copy as markdown");
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("**Bash** pnpm test\n\n```\n27 passed\n```"));
  });

  it("a tool call whose output contains a fence is fenced with a longer one", async () => {
    const user = setup();
    renderRow(rowsOf(bash("b2", "cat README.md", "```ts\nx\n```"))[0]);
    await choose(user, "Copy as markdown");
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("**Bash** cat README.md\n\n````\n```ts\nx\n```\n````"));
  });

  it("a run of tool calls copies each call, one after the other", async () => {
    const user = setup();
    const row = rowsOf([...bash("r1", "ls", "a b"), ...bash("r2", "pwd", "/w")])[0];
    expect(row.kind).toBe("toolrun");
    renderRow(row);
    await choose(user, "Copy as markdown");
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("**Bash** ls\n\n```\na b\n```\n\n**Bash** pwd\n\n```\n/w\n```"));
  });

  it("the turn footer copies its words", async () => {
    const user = setup();
    renderRow(rowsOf([agent({ type: "turn_completed", was_interrupted: false, num_turns: 1, usage: {}, turn_usage: { input_tokens: 1200, output_tokens: 380, cache_read_tokens: 0, cache_creation_tokens: 0, cost_usd: 0.03 } })])[0]);
    await choose(user, "Copy as markdown");
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("Done · 1.2k in / 380 out · $0.03"));
  });
});

describe("row menu: copy tool output", () => {
  it("copies what a tool call produced", async () => {
    const user = setup();
    renderRow(rowsOf(bash("o1", "pnpm test", "27 passed · 0 failed"))[0]);
    await choose(user, "Copy tool output");
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("27 passed · 0 failed"));
  });

  it("copies the file change a tool reported as its diff, after its result", async () => {
    const user = setup();
    const row = rowsOf([
      agent({ type: "tool_call_started", name: "Edit", call_id: "e1", input: { file_path: "a.ts" }, is_subagent: false }),
      agent({ type: "tool_call_finished", name: "Edit", call_id: "e1", ok: true, result: "Edited a.ts", parts: [{ type: "diff", path: "a.ts", old_text: "x", new_text: "y" }] }),
    ])[0];
    renderRow(row);
    await choose(user, "Copy tool output");
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("Edited a.ts\n-x\n+y"));
  });

  it("copies the outputs of every call in a run", async () => {
    const user = setup();
    renderRow(rowsOf([...bash("q1", "ls", "a"), ...bash("q2", "pwd", "/w")])[0]);
    await choose(user, "Copy tool output");
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("a\n/w"));
  });

  it("is not offered for a prompt, which has no tool output", async () => {
    renderRow(rowsOf([prompt("hi")])[0]);
    await openMenu(setup());
    expect(screen.queryByRole("menuitem", { name: "Copy tool output" })).toBeNull();
  });
});

describe("row menu: quote in reply", () => {
  it("appends the quote to the session's draft, keeping its text and its attachments", async () => {
    const user = setup();
    const file = { key: "f1", name: "shot.png", size: 3, mime: "image/png", read: async () => new Uint8Array() } as PendingFile;
    setDraft(KEY, { text: "my notes", files: [file] });
    const before = peekDraft(KEY)!.files;
    renderRow(rowsOf([prompt("line one\nline two")])[0]);
    await choose(user, "Quote in reply");
    const d = peekDraft(KEY)!;
    expect(d.text).toBe("my notes\n\n> line one\n> line two\n");
    expect(d.files).toBe(before);
  });

  it("starts the draft with the quote when there was none", async () => {
    const user = setup();
    renderRow(rowsOf([agent({ type: "assistant_delta", text: "Use `total()`.", message_id: "m" })])[0]);
    await choose(user, "Quote in reply");
    expect(peekDraft(KEY)).toEqual({ text: "> Use `total()`.\n", files: [] });
  });

  it("with a composer open, the quote goes to it (which keeps its own text) and the composer takes the focus", async () => {
    const user = setup();
    const box = document.createElement("textarea");
    box.setAttribute("aria-label", "Message");
    document.body.appendChild(box);
    const seen = vi.fn();
    const off = onQuote(KEY, seen);
    renderRow(rowsOf([prompt("quote me")])[0]);
    await choose(user, "Quote in reply");
    expect(seen).toHaveBeenCalledWith("quote me");
    expect(peekDraft(KEY)).toBeUndefined();
    expect(document.activeElement).toBe(box);
    off();
  });

  it("after the composer closes, a quote is kept as the draft again", async () => {
    const user = setup();
    const seen = vi.fn();
    onQuote(KEY, seen)();
    renderRow(rowsOf([prompt("later")])[0]);
    await choose(user, "Quote in reply");
    expect(seen).not.toHaveBeenCalled();
    expect(peekDraft(KEY)).toEqual({ text: "> later\n", files: [] });
  });

  it("is not offered for a tool call, which has no prose to quote", async () => {
    const user = setup();
    renderRow(rowsOf(bash("t1", "ls", "a"))[0]);
    await openMenu(user);
    expect(screen.queryByRole("menuitem", { name: "Quote in reply" })).toBeNull();
  });

  it("quoted blank lines stay quoted, and trailing blanks are dropped", () => {
    expect(quoteLines("a\n\nb\n\n")).toBe("> a\n>\n> b");
  });

  it("a quote is appended to a reply after a blank line; an empty reply gets just the quote", () => {
    expect(appendQuote("", "a")).toBe("> a\n");
    expect(appendQuote("  \n", "a")).toBe("> a\n");
    expect(appendQuote("my notes\n\n", "a")).toBe("my notes\n\n> a\n");
  });
});

describe("row menu: open raw event", () => {
  it("shows the pretty JSON of the events behind the item, and only those", async () => {
    const user = setup();
    const first = agent({ type: "assistant_delta", text: "Hel", message_id: "m" }, 100);
    const second = agent({ type: "assistant_delta", text: "lo", message_id: "m" }, 200);
    const other = agent({ type: "notice", message: "elsewhere" }, 300);
    const events = [first, second, other];
    app.store.set((s) => ({ ...s, transcripts: { ...s.transcripts, [KEY]: { events, hasMore: false, loading: false, earlier: { events: [], hasMore: false, loading: false } } } }));
    renderRow(rowsOf(events)[0]);
    await choose(user, "Open raw event");
    const shown = JSON.parse(screen.getByTestId("raw-json").textContent ?? "null");
    expect(shown).toEqual(JSON.parse(JSON.stringify([first, second])));
    expect(screen.getByRole("dialog", { name: /Raw events/ })).toBeInTheDocument();
  });

  it("reads the events before what is loaded from the host, and shows them too", async () => {
    const user = setup();
    const early1 = agent({ type: "assistant_delta", text: "Hel", message_id: "m" }, 100);
    const early2 = agent({ type: "assistant_delta", text: "lo", message_id: "m" }, 200);
    const loaded = agent({ type: "notice", message: "loaded" }, 300);
    app.store.set((s) => ({ ...s, transcripts: { ...s.transcripts, [KEY]: { events: [loaded], hasMore: true, loading: false, earlier: { events: [], hasMore: false, loading: false } } } }));
    const history = vi.spyOn(app.api, "history").mockResolvedValue({ events: [early1, early2] as SessionEvent[], has_more: false });
    renderRow(rowsOf([early1, early2])[0]);
    await choose(user, "Open raw event");
    await waitFor(() => expect(screen.getByTestId("raw-json")).toBeInTheDocument());
    expect(history).toHaveBeenCalledWith("h1", expect.objectContaining({ id: "s1", before: 201 }));
    expect(JSON.parse(screen.getByTestId("raw-json").textContent ?? "null")).toHaveLength(2);
  });

  it("the drawer's Copy JSON copies all of the JSON", async () => {
    const user = setup();
    const big = agent({ type: "assistant_delta", text: "x".repeat(RAW_CAP_BYTES + 5_000), message_id: "m" }, 100);
    app.store.set((s) => ({ ...s, transcripts: { ...s.transcripts, [KEY]: { events: [big], hasMore: false, loading: false, earlier: { events: [], hasMore: false, loading: false } } } }));
    renderRow(rowsOf([big])[0]);
    await choose(user, "Open raw event");
    expect(screen.getByTestId("raw-truncated")).toBeInTheDocument();
    expect(screen.getByTestId("raw-json").textContent!.length).toBe(RAW_CAP_BYTES);
    await user.click(screen.getByRole("button", { name: "Copy JSON" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    expect(writeText.mock.calls[0][0]).toBe(JSON.stringify([big], null, 2));
  });

  it("is not offered outside a session", () => {
    renderRow(rowsOf([prompt("hi")])[0], null);
    expect(screen.queryByRole("menuitem", { name: "Open raw event" })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: "Quote in reply" })).toBeNull();
  });
});

describe("raw event helpers", () => {
  const e1 = agent({ type: "notice", message: "a" }, 10);
  const e2 = agent({ type: "notice", message: "b" }, 20);
  const e3 = agent({ type: "notice", message: "c" }, 30);

  it("spanEvents keeps the cursors of the span, in order", () => {
    expect(spanEvents([e1, e2, e3], { from: 10, to: 20 }, MAIN, "live").map((e) => e.cursor)).toEqual([10, 20]);
  });

  it("the earlier history keeps the agent's events only", () => {
    const input = prompt("x", 5);
    input.cursor = 15;
    expect(spanEvents([e1, input, e2], { from: 10, to: 20 }, MAIN, "earlier").map((e) => e.cursor)).toEqual([10, 20]);
  });

  it("rawJson cuts the text at the cap and says so, and keeps the full text", () => {
    const big = agent({ type: "assistant_delta", text: "y".repeat(RAW_CAP_BYTES), message_id: "m" });
    const out = rawJson([big], RAW_CAP_BYTES);
    expect(out.truncated).toBe(true);
    expect(out.text).toHaveLength(RAW_CAP_BYTES);
    expect(out.full.length).toBeGreaterThan(RAW_CAP_BYTES);
    expect(rawJson([e1]).truncated).toBe(false);
  });

  it("the drawer says when no event lies in the item's span", () => {
    render(
      <AppProvider value={app}>
        <RawEventDrawer sessionKey={KEY} host="h1" id="s1" scope={MAIN} part="live" span={{ from: 1, to: 2 }} onClose={() => undefined} />
      </AppProvider>,
    );
    expect(screen.getByTestId("raw-empty")).toBeInTheDocument();
  });
});
