import { act, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { FleetEvent, LinkState } from "../../src/bridge/wire";
import { applyFleet } from "../../src/state/controller";
import { createApp } from "../../src/state";
import { hostStale } from "../../src/state/selectors";
import { mountApp, quietBackend } from "../helpers";

const hello = (host: string) => ({ protocol: { major: 1, minor: 0 }, build: "fixture", host_id: host, role: "daemon", features: [] });
const reconnecting: LinkState = { state: "reconnecting", attempt: 1, next_at: { secs_since_epoch: 0, nanos_since_epoch: 0 }, last_error: "timed out" };

async function boot() {
  const backend = quietBackend();
  const app = createApp(backend);
  await waitFor(() => expect(Object.keys(app.store.get().sessions).length).toBe(15));
  return { backend, app };
}

describe("sessions_live is the only source of a host's live flag", () => {
  it("a link event never changes it: a dropped link keeps the flag until the hub says otherwise", async () => {
    const { app } = await boot();
    expect(app.store.get().hosts.hz.sessionsLive).toBe(true);
    const s = applyFleet(app.store.get(), { kind: "link", host: "hz", state: reconnecting });
    expect(s.hosts.hz.sessionsLive).toBe(true);
    expect(hostStale(s, "hz")).toBe(false);
    app.dispose();
  });

  it("a sessions snapshot does not set the flag by itself", async () => {
    const { app } = await boot();
    const s0 = applyFleet(app.store.get(), { kind: "sessions_live", host: "hz", live: false });
    const s1 = applyFleet(s0, { kind: "sessions", host: "hz", sessions: [] });
    expect(s1.hosts.hz.sessionsLive).toBe(false);
    app.dispose();
  });

  it("a feed refusal does not clear it by itself (the hub sends sessions_live false with it)", async () => {
    const { app } = await boot();
    const s = applyFleet(app.store.get(), { kind: "feed_error", host: "hz", error: "refused" });
    expect(s.hosts.hz.sessionsLive).toBe(true);
    expect(hostStale(s, "hz")).toBe(true);
    app.dispose();
  });

  it("applies the event for the host it names, and only that host", async () => {
    const { app } = await boot();
    const s = applyFleet(app.store.get(), { kind: "sessions_live", host: "hz", live: false });
    expect(s.hosts.hz.sessionsLive).toBe(false);
    expect(s.hosts.x1.sessionsLive).toBe(app.store.get().hosts.x1.sessionsLive);
    const unknown = applyFleet(s, { kind: "sessions_live", host: "nope", live: true } as FleetEvent);
    expect(unknown).toBe(s);
    app.dispose();
  });

  it("the fixture's drop and reconnect report it through the hub's events, in the hub's order", async () => {
    const backend = quietBackend();
    const seen: string[] = [];
    const app = createApp(backend);
    await waitFor(() => expect(Object.keys(app.store.get().sessions).length).toBe(15));
    (backend as unknown as { fleetSubs: Set<(e: FleetEvent) => void> }).fleetSubs.add((e) => seen.push(`${e.kind}${"host" in e && e.host === "hz" ? ":hz" : ""}`));
    backend.setLink("hz", reconnecting);
    backend.setLink("hz", { state: "connected", hello: hello("hz") });
    expect(seen).toEqual(["sessions_live:hz", "link:hz", "link:hz", "sessions_live:hz", "sessions:hz"]);
    app.dispose();
  });
});

describe("a connect in progress names its stage and how long it has run", () => {
  it("the host's detail and its chip say 'Connecting · <stage> · <elapsed>'", async () => {
    const backend = quietBackend();
    mountApp("/hosts?h=hz", backend);
    act(() => backend.setLink("hz", { state: "connecting", stage: "opening ssh", since_ms: Date.now() - 5_000 }));
    const detail = await screen.findByRole("complementary", { name: /details/ });
    expect(within(detail).getAllByText(/^Connecting · opening ssh · \d+s$/).length).toBeGreaterThan(0);
    const chips = [...document.querySelectorAll<HTMLElement>(".conn[data-link='connecting']")];
    expect(chips.length).toBeGreaterThan(0);
    for (const c of chips) expect(c.textContent).toMatch(/^Connecting · opening ssh · \d+s$/);
  });

  it("falls back to a bare 'Connecting' without a stage", async () => {
    const backend = quietBackend();
    mountApp("/hosts?h=hz", backend);
    act(() => backend.setLink("hz", { state: "connecting", since_ms: Date.now() }));
    const detail = await screen.findByRole("complementary", { name: /details/ });
    expect(within(detail).getAllByText("Connecting").length).toBeGreaterThan(0);
    const chips = [...document.querySelectorAll<HTMLElement>(".conn[data-link='connecting']")];
    expect(chips.length).toBeGreaterThan(0);
    for (const c of chips) expect(c.textContent).toBe("Connecting");
  });
});
