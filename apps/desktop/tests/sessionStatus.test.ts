import { describe, it, expect } from "vitest";
import type { SessionView } from "../src/bridge/types";
import {
  isPaused,
  canResume,
  canPause,
  canSend,
  sendBlockedReason,
  sessionConnHint,
  aggregateCardStatus,
} from "../src/bridge/sessionStatus";

function sv(overrides: Partial<SessionView> = {}): SessionView {
  return {
    id: "s",
    host_id: "h",
    state: "Working",
    live: true,
    conn_health: "Connected",
    ...overrides,
  } as unknown as SessionView;
}

describe("sessionStatus — single source of truth", () => {
  it("isPaused/canResume/canPause read authoritative live, not phase", () => {
    // Done but live → NOT paused → Pause offered (this is the dashboard bug 3 fix).
    const doneLive = sv({ state: "Done", live: true });
    expect(isPaused(doneLive)).toBe(false);
    expect(canPause(doneLive)).toBe(true);
    expect(canResume(doneLive)).toBe(false);

    const paused = sv({ state: "Working", live: false });
    expect(isPaused(paused)).toBe(true);
    expect(canResume(paused)).toBe(true);
    expect(canPause(paused)).toBe(false);
  });

  it("missing live is treated as running (Resumed)", () => {
    const s = sv({ live: undefined });
    expect(isPaused(s)).toBe(false);
    expect(canPause(s)).toBe(true);
  });

  it("canSend requires running AND a clean connection", () => {
    expect(canSend(sv({ live: true, conn_health: "Connected" }))).toBe(true);
    expect(canSend(sv({ live: false, conn_health: "Connected" }))).toBe(false);
    expect(canSend(sv({ live: true, conn_health: "Reconnecting" }))).toBe(false);
    expect(canSend(sv({ live: true, conn_health: "Unstable" }))).toBe(false);
    expect(canSend(sv({ live: true, conn_health: "Disconnected" }))).toBe(false);
    // missing conn_health treated as Connected
    expect(canSend(sv({ live: true, conn_health: undefined }))).toBe(true);
  });

  it("sendBlockedReason explains each block, null when sendable", () => {
    expect(sendBlockedReason(sv())).toBeNull();
    expect(sendBlockedReason(sv({ live: false }))).toMatch(/paused/i);
    expect(sendBlockedReason(sv({ conn_health: "Reconnecting" }))).toMatch(/reconnect/i);
    expect(sendBlockedReason(sv({ conn_health: "Unstable" }))).toMatch(/unstable/i);
    expect(sendBlockedReason(sv({ conn_health: "Disconnected" }))).toMatch(/disconnect/i);
  });

  it("sessionConnHint prefers per-session conn_health over host status", () => {
    const hosts = [{ id: "h", status: "Connected" }];
    expect(sessionConnHint(sv({ conn_health: "Unstable" }), hosts)).toBe("Unstable");
    expect(sessionConnHint(sv({ conn_health: "Reconnecting" }), hosts)).toBe("Reconnecting");
    expect(sessionConnHint(sv({ conn_health: "Connected" }), hosts)).toBeNull();
    // falls back to host status when session is clean
    const down = [{ id: "h", status: "Unreachable" }];
    expect(sessionConnHint(sv({ conn_health: "Connected" }), down)).toBe("Disconnected");
  });

  it("a PAUSED session never shows a conn hint — pause is not reconnecting", () => {
    // The real bug: a manually-paused session flashed "Reconnecting". Liveness-first:
    // once live=false there is nothing to reconnect to, so no hint — even if a stale
    // conn_health raced the pause OR the host still reads Connecting/Unreachable.
    const connecting = [{ id: "h", status: "Connecting" }];
    expect(sessionConnHint(sv({ live: false, conn_health: "Reconnecting" }), connecting)).toBeNull();
    expect(sessionConnHint(sv({ live: false, conn_health: "Unstable" }), connecting)).toBeNull();
    const down = [{ id: "h", status: "Unreachable" }];
    expect(sessionConnHint(sv({ live: false, conn_health: "Connected" }), down)).toBeNull();
    // aggregate over all-paused members → no hint, regardless of member conn_health.
    const agg = aggregateCardStatus(
      [sv({ id: "a", live: false, conn_health: "Reconnecting" }), sv({ id: "b", live: false, conn_health: "Unstable" })],
      down
    );
    expect(agg.connHint).toBeNull();
  });

  it("aggregateCardStatus: Pause if ANY member live; worst conn hint wins", () => {
    const hosts = [{ id: "h", status: "Connected" }];
    // one live + one paused → card can Pause (some live).
    const a = aggregateCardStatus(
      [sv({ id: "a", live: true }), sv({ id: "b", live: false })],
      hosts
    );
    expect(a.canPause).toBe(true);
    expect(a.canResume).toBe(false);

    // all paused → Resume.
    const b = aggregateCardStatus(
      [sv({ id: "a", live: false }), sv({ id: "b", live: false })],
      hosts
    );
    expect(b.canPause).toBe(false);
    expect(b.canResume).toBe(true);

    // worst hint: Unstable beats Reconnecting.
    const c = aggregateCardStatus(
      [
        sv({ id: "a", conn_health: "Reconnecting" }),
        sv({ id: "b", conn_health: "Unstable" }),
      ],
      hosts
    );
    expect(c.connHint).toBe("Unstable");

    // empty members → no hint, resume default.
    const e = aggregateCardStatus([], hosts);
    expect(e.connHint).toBeNull();
    expect(e.canResume).toBe(true);
  });
});
