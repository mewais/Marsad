import type { CompactionInfo, ModelUsage, RateLimitInfo, SessionEvent } from "../../../bridge/wire";

export interface UsageFacts {
  perModel: Record<string, ModelUsage> | null;
  compactions: { t: number; info: CompactionInfo }[];
  rateLimits: { t: number; info: RateLimitInfo }[];
}

/** Facts the snapshot does not carry, read from the session's events (latest per-model split, notices). */
export function usageFacts(events: SessionEvent[]): UsageFacts {
  let perModel: UsageFacts["perModel"] = null;
  const compactions: UsageFacts["compactions"] = [];
  const rateLimits: UsageFacts["rateLimits"] = [];
  for (const e of events) {
    if (e.kind !== "agent") continue;
    const a = e.event;
    if (a.type === "turn_completed" && a.per_model && Object.keys(a.per_model).length) perModel = a.per_model;
    else if (a.type === "compaction") compactions.push({ t: e.t, info: a.info });
    else if (a.type === "rate_limit_notice") rateLimits.push({ t: e.t, info: a.info });
  }
  return { perModel, compactions, rateLimits };
}
