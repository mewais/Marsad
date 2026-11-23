import { describe, expect, it } from "vitest";
import type { Api } from "../../src/bridge/api";
import { clockOffset, clockWords, measureClock } from "../../src/features/diagnostics/clock";

describe("measuring a host's clock", () => {
  it("takes the host's reading to be the middle of the round trip", () => {
    // Sent at 1000, answered 40 ms later; the host read 3520: its clock was 3520 - 1020 = 2500 ms ahead.
    expect(clockOffset(1000, 40, 3520)).toEqual({ offsetMs: 2500, rttMs: 40 });
    expect(clockOffset(1000, 40, 1020)).toEqual({ offsetMs: 0, rttMs: 40 });
    expect(clockOffset(1000, 40, -1980)).toEqual({ offsetMs: -3000, rttMs: 40 });
    // A clock that ran backwards during the exchange never gives a negative round trip.
    expect(clockOffset(1000, -5, 1000).rttMs).toBe(0);
  });

  it("makes exactly one request, to the host, and measures it with the clocks it is given", async () => {
    const calls: string[] = [];
    const api = { diagSources: async (h: string) => (calls.push(h), { now_ms: 12_520, boot: "b", sources: [] }) } as unknown as Api;
    let mono = 100;
    const o = await measureClock(api, "hz", () => 10_000, () => (mono += 40));
    expect(calls).toEqual(["hz"]);
    // wall 10000, round trip 40 ms (the monotonic clock moved by 40 between its two reads): 12520 - 10020.
    expect(o).toEqual({ offsetMs: 2500, rttMs: 40 });
  });

  it("says the offset, the round trip and that the times are still approximate", () => {
    const ready = (offsetMs: number, rttMs: number) => clockWords({ state: "ready", offset: { offsetMs, rttMs } }, "hetzner");
    expect(ready(2500, 12)).toBe("hetzner's clock is 2.5 s ahead of this computer's (measured once when this tab opened; round trip 12 ms). Its times are corrected by that and are still approximate.");
    expect(ready(-90, 30)).toContain("90 ms behind this computer's");
    expect(ready(0, 3)).toContain("the same as this computer's");
    expect(clockWords({ state: "measuring" }, "hetzner")).toBe("Measuring hetzner's clock offset…");
    expect(clockWords({ state: "failed", reason: "timed out" }, "hetzner")).toBe("hetzner's clock could not be read (timed out); its times are shown as the host wrote them.");
  });
});
