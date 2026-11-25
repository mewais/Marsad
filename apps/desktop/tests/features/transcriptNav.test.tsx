import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Spec } from "../../src/bridge/fixture/sessions";
import { quietBackend, renderApp, sessionByName } from "../helpers";
import { holdHistory, type HistoryGate } from "../historyGate";
import { layoutTranscript, scrollTranscript, transcript } from "../transcriptLayout";

/** Prompts, assistant prose and tool rows, in rounds: a hundred of them (about 750 events), all loaded. */
const SHORT: Spec = { n: 903, name: "nav-short-session", driver: "claude", host: "x1", proj: "ledgerline", branch: "main", agoMin: 1, cost: 1, kind: "idle", line: "", rounds: 100 };
/** A long session, for the outline: the prompts of its older rounds are not loaded. */
const LONG: Spec = { ...SHORT, n: 904, name: "nav-long-session", rounds: 400 };
const prompt = (i: number) => `Index and summarise module ${i} of ledgerline.`;

let unlayout: () => void;
beforeEach(() => {
  unlayout = layoutTranscript(400);
});
afterEach(() => unlayout());

async function open(spec: Spec, gateAfter?: number) {
  const b = quietBackend({ specs: [spec] });
  let gate: HistoryGate | null = null;
  if (gateAfter !== undefined) gate = holdHistory(b, gateAfter);
  const s = sessionByName(b, spec.name);
  await renderApp(`/session/${s.host}/${s.id}`, b);
  await screen.findByRole("heading", { name: spec.name });
  await waitFor(() => expect(transcript()).not.toBeNull());
  return { b, gate, s };
}

/** Whether module `i`'s prompt is a row of the transcript (the outline lists prompts too, so the list is searched, not the page). */
const inTranscript = (i: number) => transcript().textContent?.includes(prompt(i)) ?? false;

const callRow = (file: string) => screen.getByRole("button", { name: new RegExp(file.replace(/\./g, "\\.")) });

describe("Alt+Up and Alt+Down move between the prompts of the loaded transcript", () => {
  it("from the top, Alt+Up goes to the newest prompt, then to the one before it, and Alt+Down comes back; other rows are passed over", async () => {
    const user = userEvent.setup();
    await open(SHORT);
    // Up to the first rows: the prompts of the start are in view, the newest are not.
    scrollTranscript(0);
    await waitFor(() => expect(inTranscript(99)).toBe(false));
    await user.keyboard("{Alt>}{ArrowUp}{/Alt}");
    await waitFor(() => expect(inTranscript(99)).toBe(true));
    // The prompt before module 99 is module 98: the answer and tool rows between them are passed over.
    await user.keyboard("{Alt>}{ArrowUp}{/Alt}");
    await waitFor(() => expect(inTranscript(98)).toBe(true));
    await user.keyboard("{Alt>}{ArrowDown}{/Alt}");
    await waitFor(() => expect(inTranscript(99)).toBe(true));
  });
});

describe("Collapse all / Expand all", () => {
  it("expands and collapses every tool row of the transcript; a row toggled afterwards keeps its own state", async () => {
    const user = userEvent.setup();
    await open(SHORT);
    // Modules 99 and 97 are single read rows (no bash run beside them), at the bottom where they are mounted.
    expect(callRow("src/module-99.ts")).toHaveAttribute("aria-expanded", "false");
    await user.click(screen.getByRole("button", { name: "Expand all" }));
    await waitFor(() => expect(callRow("src/module-99.ts")).toHaveAttribute("aria-expanded", "true"));
    expect(callRow("src/module-97.ts")).toHaveAttribute("aria-expanded", "true");
    // Toggle one row closed: it stays closed, the others stay open.
    await user.click(callRow("src/module-99.ts"));
    expect(callRow("src/module-99.ts")).toHaveAttribute("aria-expanded", "false");
    expect(callRow("src/module-97.ts")).toHaveAttribute("aria-expanded", "true");
    await user.click(screen.getByRole("button", { name: "Collapse all" }));
    await waitFor(() => expect(callRow("src/module-97.ts")).toHaveAttribute("aria-expanded", "false"));
    expect(callRow("src/module-99.ts")).toHaveAttribute("aria-expanded", "false");
  });
});

describe("Outline: the prompts of the conversation, each jumps to its prompt", () => {
  it("lists prompts with their time and text, and a click on one that is not loaded pages back to it", async () => {
    const user = userEvent.setup();
    const { gate } = await open(LONG, 2);

    await user.click(screen.getByRole("button", { name: "Outline" }));
    const outline = await screen.findByRole("complementary", { name: "Outline" });
    const item = await within(outline).findByRole("button", { name: new RegExp(prompt(300).replace(/\./g, "\\.")) });
    expect(item.textContent).toMatch(/\d\d:\d\d/);
    expect(inTranscript(300)).toBe(false);
    await user.click(item);
    for (let i = 0; i < 30 && !inTranscript(300); i++) {
      await waitFor(() => expect(gate!.pending().length).toBeGreaterThan(0));
      await act(async () => gate!.release());
    }
    expect(inTranscript(300)).toBe(true);
  });
});
