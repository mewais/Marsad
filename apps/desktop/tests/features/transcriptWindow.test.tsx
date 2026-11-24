import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { EARLIER_SPECS } from "../../src/bridge/fixture/earlier";
import { SPECS } from "../../src/bridge/fixture/sessions";
import { quietBackend, renderApp, sessionByName } from "../helpers";
import { holdHistory } from "../historyGate";
import { layoutTranscript } from "../transcriptLayout";

// The transcript is one virtual list (`.transcript`): its layout is mocked to a viewport of VIEW pixels.
const VIEW = 400;
const LONG = { ...EARLIER_SPECS[0], n: 60, name: "resumed-long", longHistory: true };

let unlayout: () => void;
beforeEach(() => {
  unlayout = layoutTranscript(VIEW);
});
afterEach(() => unlayout());

describe("the transcript is one virtual list that pages earlier history when its start is reached", () => {
  it("one list for the whole transcript; each reach of its start asks for one earlier page, and staying at the start asks for none", async () => {
    const b = quietBackend({ specs: [...SPECS, ...EARLIER_SPECS, LONG] });
    const gate = holdHistory(b, Number.POSITIVE_INFINITY);
    const s = sessionByName(b, LONG.name);
    await renderApp(`/session/${s.host}/${s.id}`, b);
    await screen.findByRole("heading", { name: LONG.name });
    const tr = (await waitFor(() => {
      const el = document.querySelector(".transcript");
      expect(el).not.toBeNull();
      return el;
    })) as HTMLElement;
    expect(document.querySelectorAll(".transcript")).toHaveLength(1);
    expect(tr).toHaveClass("vlist");
    // Opened at the bottom, the transcript asks for nothing.
    const asked = () => gate.answered().length;
    const start = asked();
    // The reader scrolling to the start: a wheel gesture (so a following list releases), then the move and the scroll event.
    const reach = (top: number) =>
      act(() => {
        fireEvent.wheel(tr);
        tr.scrollTop = top;
        fireEvent.scroll(tr);
      });
    reach(0);
    await waitFor(() => expect(asked()).toBe(start + 1));
    // Still at the start: the same reach, no second request.
    reach(0);
    reach(0);
    await act(async () => {});
    expect(asked()).toBe(start + 1);
    // Away from the start, then back: a new reach.
    reach(200);
    reach(0);
    await waitFor(() => expect(asked()).toBe(start + 2));
  });
});
