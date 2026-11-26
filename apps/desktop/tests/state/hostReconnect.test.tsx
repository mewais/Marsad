import { waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { createApp } from "../../src/state";
import { applyFleet } from "../../src/state/controller";
import { MAIN } from "../../src/state/scope";
import { hostStale } from "../../src/state/selectors";
import { quietBackend, sessionByName } from "../helpers";

const hello = (host: string) => ({ protocol: { major: 1, minor: 0 }, build: "fixture", host_id: host, role: "daemon", features: [] });
const reconnecting = { state: "reconnecting" as const, attempt: 1, next_at: { secs_since_epoch: 0, nanos_since_epoch: 0 }, last_error: "timed out" };

async function boot() {
  const backend = quietBackend();
  const app = createApp(backend);
  await waitFor(() => expect(Object.keys(app.store.get().sessions).length).toBe(15));
  return { backend, app };
}

describe("a host whose link comes back", () => {
  it("stays last known after its link connects, and is live only once its session snapshot arrives", async () => {
    const { app } = await boot();
    const hzSessions = Object.entries(app.store.get().sessions)
      .filter(([k]) => k.startsWith("hz/"))
      .map(([, v]) => v);
    expect(hzSessions.length).toBeGreaterThan(0);
    // The hub ends the host's live sessions as its link drops (its own event, before the link's).
    let s = applyFleet(app.store.get(), { kind: "sessions_live", host: "hz", live: false });
    s = applyFleet(s, { kind: "link", host: "hz", state: reconnecting });
    expect(hostStale(s, "hz")).toBe(true);
    // The link connecting alone must not make the old list look live: the hub sends a fresh snapshot after it resubscribes.
    s = applyFleet(s, { kind: "link", host: "hz", state: { state: "connected", hello: hello("hz") } });
    expect(hostStale(s, "hz")).toBe(true);
    s = applyFleet(s, { kind: "sessions_live", host: "hz", live: true });
    s = applyFleet(s, { kind: "sessions", host: "hz", sessions: hzSessions });
    expect(hostStale(s, "hz")).toBe(false);
    // Only that host: another host's list is not touched by hz's link.
    expect(hostStale(s, "x1")).toBe(false);
    app.dispose();
  });

  it("restarts the ended session feeds of that host, and leaves the other hosts' ended feeds alone", async () => {
    const { backend, app } = await boot();
    const a = sessionByName(backend, "pdf-renderer-worker"); // on hz
    const b = sessionByName(backend, "migrate-auth-middleware"); // on this machine (x1)
    expect(a.host).toBe("hz");
    expect(b.host).toBe("x1");
    const offA = app.transcripts.open(a.key, MAIN);
    const offB = app.transcripts.open(b.key, MAIN);
    await waitFor(() => expect(app.store.get().transcripts[a.key]?.events.length).toBeGreaterThan(0));
    await waitFor(() => expect(app.store.get().transcripts[b.key]?.events.length).toBeGreaterThan(0));

    // Both session feeds end for good (the daemon closed them).
    backend.endSubscriptions(`session.events:${a.id}`);
    backend.endSubscriptions(`session.events:${b.id}`);
    await waitFor(() => expect(app.store.get().feedErrors).toHaveProperty([`events:${a.key}`]));
    expect(app.store.get().feedErrors).toHaveProperty([`events:${b.key}`]);

    // hz drops and comes back: its feed starts again; the hub link never moved.
    backend.setLink("hz", reconnecting);
    backend.setLink("hz", { state: "connected", hello: hello("hz") });
    await waitFor(() => expect(app.store.get().feedErrors).not.toHaveProperty([`events:${a.key}`]));
    expect(app.store.get().feedErrors).toHaveProperty([`events:${b.key}`]);
    offA();
    offB();
    app.dispose();
  });
});
