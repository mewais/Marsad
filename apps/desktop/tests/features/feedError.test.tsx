import { act, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { FleetEvent } from "../../src/bridge/wire";
import { quietBackend, renderApp } from "../helpers";

/** The fixture's fleet feed is private; a test drives it the way the hub would. */
const emitFleet = (backend: ReturnType<typeof quietBackend>, e: FleetEvent) =>
  (backend as unknown as { fleet(e: FleetEvent): void }).fleet(e);

describe("a host whose session list the hub refused", () => {
  it("says so in one sentence on its Hosts detail, and clears when its list comes through again", async () => {
    const backend = quietBackend();
    const { history } = await renderApp("/inbox", backend);
    act(() => emitFleet(backend, { kind: "feed_error", host: "hz", error: "the daemon refused the topic: not allowed.\nSee the log." }));
    act(() => history.push("/hosts?h=hz"));
    const note = await screen.findByTestId("host-feed-error");
    expect(note).toHaveTextContent(
      "Marsad stopped receiving this host's session list: the daemon refused the topic: not allowed. See the log. It will resume when the host reconnects.",
    );
    expect(note.textContent).not.toMatch(/\.\./);
    act(() => emitFleet(backend, { kind: "sessions", host: "hz", sessions: [] }));
    await waitFor(() => expect(screen.queryByTestId("host-feed-error")).toBeNull());
  });

  it("is named in the Inbox's compact note, next to hosts whose link is down", async () => {
    const backend = quietBackend();
    await renderApp("/inbox", backend);
    act(() => emitFleet(backend, { kind: "feed_error", host: "hz", error: "connection reset" }));
    const group = await screen.findByTestId("waiting-hosts");
    expect(within(group).getByText(/Session list stopped:/)).toHaveTextContent("hetzner-ax41");
    expect(group).not.toHaveTextContent("connection reset");
  });
});
