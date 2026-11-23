import type { Api } from "../../bridge/api";
import { errorMessage } from "../../bridge/api";
import type { HostId } from "../../bridge/wire";
import { duration } from "./words";

/** How far a host's wall clock is from this computer's, measured once: `offsetMs` is host minus here (positive: the host is ahead). */
export interface ClockOffset {
  offsetMs: number;
  /** The round trip of the request that measured it: the offset is uncertain by up to half of it. */
  rttMs: number;
}

/** Where the measurement of a host's clock is. */
export type ClockState = { state: "measuring" } | { state: "ready"; offset: ClockOffset } | { state: "failed"; reason: string };

/**
 * The offset from one exchange: the host read its clock (`hostNowMs`) somewhere between our send at
 * `sentMs` and its answer `rttMs` later, taken to be the middle.
 */
export function clockOffset(sentMs: number, rttMs: number, hostNowMs: number): ClockOffset {
  const rtt = Math.max(0, rttMs);
  return { offsetMs: Math.round(hostNowMs - (sentMs + rtt / 2)), rttMs: Math.round(rtt) };
}

/** Ask the host's daemon what time it is (`diag.sources` answers `now_ms`) and measure the exchange. One request, no polling. */
export async function measureClock(api: Api, host: HostId, wall: () => number = Date.now, mono: () => number = () => performance.now()): Promise<ClockOffset> {
  const sent = wall();
  const started = mono();
  const { now_ms } = await api.diagSources(host);
  return clockOffset(sent, mono() - started, now_ms);
}

/** The measured state as a sentence about `hostName`'s clock. */
export function clockWords(state: ClockState, hostName: string): string {
  if (state.state === "measuring") return `Measuring ${hostName}'s clock offset…`;
  if (state.state === "failed") return `${hostName}'s clock could not be read (${state.reason}); its times are shown as the host wrote them.`;
  const { offsetMs, rttMs } = state.offset;
  const how = Math.abs(offsetMs) < 1 ? "the same as" : `${duration(Math.abs(offsetMs))} ${offsetMs > 0 ? "ahead of" : "behind"}`;
  return `${hostName}'s clock is ${how} this computer's (measured once when this tab opened; round trip ${duration(rttMs)}). Its times are corrected by that and are still approximate.`;
}

export const clockFailure = (e: unknown): ClockState => ({ state: "failed", reason: errorMessage(e) });
