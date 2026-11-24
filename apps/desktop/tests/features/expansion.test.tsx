import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { SessionEvent } from "../../src/bridge/wire";
import { createExpansion, expansionFor, ExpansionCtx } from "../../src/features/session/expansion";
import { TranscriptItem } from "../../src/features/session/TranscriptView";
import { groupToolRuns, type RowItem } from "../../src/features/session/toolRuns";
import { createTranscriptFold } from "../../src/features/session/transcript";
import { MAIN } from "../../src/state/scope";
import { renderApp, sessionByName, quietBackend } from "../helpers";

let n = 0;
const ev = (e: Record<string, unknown>) => ({ cursor: ++n * 65536, t: n, ...e }) as unknown as SessionEvent;
const agent = (event: unknown) => ev({ kind: "agent", event });
const call = (id: string) => agent({ type: "tool_call_started", name: "Read", call_id: id, input: { file_path: `${id}.ts` }, is_subagent: false });
const done = (id: string) => agent({ type: "tool_call_finished", name: "Read", call_id: id, ok: true, result: `result ${id}` });
const think = (text: string) => agent({ type: "thinking_delta", text, message_id: "th" });
const rowsOf = (events: SessionEvent[]): RowItem[] => groupToolRuns(createTranscriptFold(MAIN).update(events));

/** Draws the rows inside a transcript's expansion, the way TranscriptList does. */
function mount(rows: readonly RowItem[], store: ReturnType<typeof expansionFor> | null) {
  const list = rows.map((r) => <TranscriptItem key={r.key} item={r} />);
  return render(store ? <ExpansionCtx.Provider value={store}>{list}</ExpansionCtx.Provider> : <>{list}</>);
}
/** The header of a tool call, by the file it names. */
const callRow = (file: string) => screen.getByRole("button", { name: new RegExp(file) });

describe("expansion store", () => {
  it("a row is closed until it is opened, and a toggle flips only that row", () => {
    const e = createExpansion();
    expect(e.isOpen("tool:1")).toBe(false);
    e.set("tool:1", true);
    expect(e.isOpen("tool:1")).toBe(true);
    expect(e.isOpen("tool:2")).toBe(false);
  });

  it("notifies subscribers when a row changes, and not when nothing does", () => {
    const e = createExpansion();
    let calls = 0;
    const off = e.subscribe(() => calls++);
    e.set("a", true);
    e.set("a", true);
    expect(calls).toBe(1);
    off();
    e.set("a", false);
    expect(calls).toBe(1);
  });

  it("setAll sets every row to the transcript-wide mode; a row toggled afterwards keeps its own state", () => {
    const e = createExpansion();
    e.set("a", true);
    e.setAll(false);
    expect(e.isOpen("a")).toBe(false);
    e.setAll(true);
    expect(e.isOpen("a")).toBe(true);
    expect(e.isOpen("never-seen")).toBe(true);
    e.set("never-seen", false);
    expect(e.isOpen("never-seen")).toBe(false);
    expect(e.isOpen("a")).toBe(true);
  });

  it("each transcript has one store for its key, and the same store comes back for it", () => {
    const a = expansionFor("host/s1|main");
    expect(expansionFor("host/s1|main")).toBe(a);
    expect(expansionFor("host/s2|main")).not.toBe(a);
  });
});

describe("expansion survives unmount", () => {
  it("a tool row opened in a transcript is open again after the row is unmounted and mounted again", async () => {
    const user = userEvent.setup();
    const store = expansionFor("t-tool-remount");
    // A remount shows the same events, so the same rows (same keys).
    const events = [call("a"), done("a")];
    const first = mount(rowsOf(events), store);
    await user.click(callRow("a.ts"));
    expect(callRow("a.ts")).toHaveAttribute("aria-expanded", "true");
    first.unmount();
    mount(rowsOf(events), store);
    expect(callRow("a.ts")).toHaveAttribute("aria-expanded", "true");
  });

  it("a thinking row and a tool run keep their state the same way", async () => {
    const user = userEvent.setup();
    const store = expansionFor("t-think-run-remount");
    // Thinking between two calls belongs to the run.
    const events = [call("a"), think("hmm"), call("b"), done("a"), done("b")];
    const first = mount(rowsOf(events), store);
    await user.click(screen.getByRole("button", { name: /2 tool calls/ }));
    await user.click(screen.getByRole("button", { name: /Thinking/ }));
    expect(screen.getByRole("button", { name: /2 tool calls/ })).toHaveAttribute("aria-expanded", "true");
    first.unmount();
    mount(rowsOf(events), store);
    expect(screen.getByRole("button", { name: /2 tool calls/ })).toHaveAttribute("aria-expanded", "true");
    expect(within(screen.getByTestId("tool-run")).getByRole("button", { name: /Thinking/ })).toHaveAttribute("aria-expanded", "true");
  });

  it("a row shown without a transcript keeps its own state, and a different transcript does not share it", async () => {
    const user = userEvent.setup();
    const rows = rowsOf([call("a"), done("a")]);
    const alone = mount(rows, null);
    await user.click(callRow("a.ts"));
    expect(callRow("a.ts")).toHaveAttribute("aria-expanded", "true");
    alone.unmount();
    mount(rows, expansionFor("t-other-transcript"));
    expect(callRow("a.ts")).toHaveAttribute("aria-expanded", "false");
  });

  it("a run and the first call inside it do not share their expansion", async () => {
    const user = userEvent.setup();
    const rows = rowsOf([call("a"), done("a"), call("b"), done("b")]);
    mount(rows, expansionFor("t-run-and-first"));
    await user.click(screen.getByRole("button", { name: /2 tool calls/ }));
    expect(screen.getByRole("button", { name: /2 tool calls/ })).toHaveAttribute("aria-expanded", "true");
    expect(callRow("a.ts")).toHaveAttribute("aria-expanded", "false");
  });

  it("the transcript-wide mode opens rows that were never touched, and a row toggled after it wins", async () => {
    const user = userEvent.setup();
    const store = expansionFor("t-mode");
    mount(rowsOf([call("a"), done("a"), think("x")]), store);
    act(() => store.setAll(true));
    expect(callRow("a.ts")).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: /Thinking/ })).toHaveAttribute("aria-expanded", "true");
    await user.click(callRow("a.ts"));
    expect(callRow("a.ts")).toHaveAttribute("aria-expanded", "false");
    act(() => store.setAll(false));
    expect(callRow("a.ts")).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByRole("button", { name: /Thinking/ })).toHaveAttribute("aria-expanded", "false");
  });
});

describe("a session's transcript keeps what was expanded when the reader leaves and comes back", () => {
  it("a tool call opened in the session view is open after navigating away and back, and the footer says the turn ended", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const s = sessionByName(backend, "docs-search-index");
    const { history } = await renderApp(`/session/${s.host}/${s.id}`, backend);
    const call = () => screen.findByRole("button", { name: /module-0\.ts/ });
    await user.click(await call());
    expect(await call()).toHaveAttribute("aria-expanded", "true");
    expect(screen.getAllByTestId("turn-footer").map((f) => f.textContent)).toEqual([expect.stringMatching(/^Done( in \d+[smh].*)?$/)]);
    // Away from the session: its transcript is not on screen.
    act(() => history.push("/inbox"));
    await waitFor(() => expect(screen.queryByRole("button", { name: /module-0\.ts/ })).toBeNull());
    act(() => history.push(`/session/${s.host}/${s.id}`));
    expect(await call()).toHaveAttribute("aria-expanded", "true");
  });
});
