import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Spec } from "../../src/bridge/fixture/sessions";
import { quietBackend, renderApp, sessionByName } from "../helpers";
import { holdHistory, type HistoryGate } from "../historyGate";
import { layoutTranscript, transcript } from "../transcriptLayout";

/*
 * Find in the transcript (B6d). A session of 950 rounds (over 6000 events). The background pager loads the newest 4000 events (the
 * resident window: the open page and eight pages of 500, which the gate passes); every history request after those is held, so
 * paging back is stepped one page at a time by the test. The oldest prompts are beyond the window and beyond the paging budget.
 */
const SPEC: Spec = { n: 902, name: "find-long-session", driver: "claude", host: "x1", proj: "ledgerline", branch: "main", agoMin: 1, cost: 1, kind: "idle", line: "", rounds: 950 };
/** The history requests the background pager makes before it stops at the resident window: the open page, then eight pages. */
const RESIDENT_PASS = 9;
const prompt = (i: number) => `Index and summarise module ${i} of ledgerline.`;

/** A stand-in for the browser's custom highlights: records what the find code asks to highlight. */
class FakeHighlight {
  ranges: Range[];
  constructor(...ranges: Range[]) {
    this.ranges = ranges;
  }
}
const highlights = new Map<string, FakeHighlight>();

let unlayout: () => void;
beforeEach(() => {
  unlayout = layoutTranscript(400);
  highlights.clear();
  Object.defineProperty(globalThis, "CSS", { configurable: true, value: { highlights: { set: (k: string, v: FakeHighlight) => void highlights.set(k, v), get: (k: string) => highlights.get(k), delete: (k: string) => highlights.delete(k) } } });
  Object.defineProperty(globalThis, "Highlight", { configurable: true, value: FakeHighlight });
});
afterEach(() => {
  unlayout();
  delete (globalThis as { CSS?: unknown }).CSS;
  delete (globalThis as { Highlight?: unknown }).Highlight;
});

async function openLong(pass: number) {
  const b = quietBackend({ specs: [SPEC] });
  const gate = holdHistory(b, pass);
  const s = sessionByName(b, SPEC.name);
  await renderApp(`/session/${s.host}/${s.id}`, b);
  await screen.findByRole("heading", { name: SPEC.name });
  await waitFor(() => expect(transcript()).not.toBeNull());
  return { b, gate, s };
}

/** Answers the held history requests one at a time until `done` holds. */
async function releaseUntil(gate: HistoryGate, done: () => boolean) {
  for (let i = 0; i < 30 && !done(); i++) {
    await waitFor(() => {
      if (!done()) expect(gate.pending().length).toBeGreaterThan(0);
    });
    if (!done()) await act(async () => gate.release());
  }
  expect(done()).toBe(true);
}

const status = () => screen.getByRole("status", { name: "Find status" });
const findInput = () => screen.getByRole("textbox", { name: "Find in transcript" });

describe("find bar: opens, streams hits, steps, and jumps to a loaded hit", () => {
  it("Ctrl+F opens the bar; hits are counted newest first; Enter and Shift+Enter step; the hit's row is in the transcript and its text is highlighted", async () => {
    const user = userEvent.setup();
    await openLong(RESIDENT_PASS);
    await user.keyboard("{Control>}f{/Control}");
    await user.type(findInput(), "summarise module 94");
    // Modules 94 and 940 to 949: eleven prompts, the newest first (the newest is in the resident window).
    await waitFor(() => expect(status()).toHaveTextContent("1 of 11"));
    expect(screen.getByText(prompt(949))).toBeInTheDocument();
    await user.keyboard("{Enter}");
    await waitFor(() => expect(status()).toHaveTextContent("2 of 11"));
    await user.keyboard("{Shift>}{Enter}{/Shift}");
    await waitFor(() => expect(status()).toHaveTextContent("1 of 11"));
    // The active hit's match is the current highlight, on the row that holds it.
    const current = highlights.get("find-current");
    expect(current?.ranges[0]?.toString()).toBe("summarise module 94");
    expect(current?.ranges[0]?.startContainer.textContent).toContain(prompt(949));
  });

  it("a hit older than the window pages back, one page at a time, until its row is loaded", async () => {
    const user = userEvent.setup();
    const { gate } = await openLong(RESIDENT_PASS);
    const answered = gate.answered().length;
    await user.keyboard("{Control>}f{/Control}");
    await user.type(findInput(), "summarise module 300 of");
    await waitFor(() => expect(status()).toHaveTextContent("1 of 1"));
    // Not loaded yet: the page before it is asked for, and held.
    await releaseUntil(gate, () => screen.queryByText(prompt(300)) !== null);
    expect(gate.answered().length).toBeGreaterThan(answered);
    expect(screen.queryByText(/Viewing older part/)).toBeNull();
    expect(highlights.get("find-current")?.ranges[0]?.toString()).toBe("summarise module 300 of");
  });

  it("a hit beyond the paging budget (2000 events) opens a detached read-only window around it, and Back to latest returns", async () => {
    const user = userEvent.setup();
    const { gate } = await openLong(RESIDENT_PASS);
    await user.keyboard("{Control>}f{/Control}");
    await user.type(findInput(), "summarise module 0 of");
    await waitFor(() => expect(status()).toHaveTextContent("1 of 1"));
    await releaseUntil(gate, () => screen.queryByText(/Viewing older part/) !== null && screen.queryByText(prompt(0)) !== null);
    expect(screen.getByRole("button", { name: "Back to latest" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Back to latest" }));
    await waitFor(() => expect(screen.queryByText(/Viewing older part/)).toBeNull());
    expect(screen.getByText(prompt(949))).toBeInTheDocument();
  });

  it("Escape closes the bar and clears the highlights", async () => {
    const user = userEvent.setup();
    await openLong(RESIDENT_PASS);
    await user.keyboard("{Control>}f{/Control}");
    await user.type(findInput(), "summarise module 94");
    await waitFor(() => expect(highlights.get("find-current")).toBeDefined());
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("textbox", { name: "Find in transcript" })).toBeNull();
    expect(highlights.size).toBe(0);
  });
});
