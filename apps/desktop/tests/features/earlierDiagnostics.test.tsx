import { act, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { EARLIER_SPECS } from "../../src/bridge/fixture/earlier";
import { SPECS } from "../../src/bridge/fixture/sessions";
import { quietBackend, renderApp, sessionByName } from "../helpers";

/** The session's Diagnostics tab (Developer mode): its trail names every move of the earlier history. */
async function open(name: string) {
  const backend = quietBackend({ specs: [...SPECS, ...EARLIER_SPECS] });
  backend.hub.settings["developer.mode"] = true;
  const s = sessionByName(backend, name);
  await renderApp(`/session/${s.host}/${s.id}?tab=diagnostics`, backend);
  await screen.findByRole("heading", { name });
  const list = await within(await screen.findByTestId("session-diagnostics")).findByRole("list", { name: "Session trail" });
  return { backend, s, list };
}

describe("the session's Diagnostics timeline and the earlier history", () => {
  it("says each move of a history that is arriving, and how it ended", async () => {
    const { backend, s, list } = await open("resumed-stripe-webhook");
    expect(await within(list).findByText("Earlier history is loading: 40 items so far, read from the agent's own storage")).toBeInTheDocument();
    const handle = backend.earlier(s.host, s.id);
    act(() => void handle.step());
    expect(await within(list).findByText("Earlier history is loading: 80 items so far, read from the agent's own storage")).toBeInTheDocument();
    act(() => void handle.step());
    act(() => void handle.step());
    expect(await within(list).findByText("Earlier history loaded: 120 items (with gaps: 120 older omitted, 2 shortened, 3 not understood), read from the agent's own storage")).toBeInTheDocument();
  });

  it("says a history that arrived whole", async () => {
    const whole = await open("resumed-invoice-rounding");
    expect(await within(whole.list).findByText(/^Earlier history loaded: \d+ items \(complete\), read from the agent's own storage$/)).toBeInTheDocument();
  });

  it("says why a history is not available", async () => {
    const { list } = await open("resumed-orphan-notes");
    const row = (await within(list).findByText("Earlier history is not available — the agent's file for the conversation does not exist, read from the agent's own storage")).closest("[role=listitem]")!;
    expect(row).toHaveAttribute("data-tone", "warn");
  });
});
