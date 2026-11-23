import type { ModelUsage, SessionSnapshot, Usage } from "../../bridge/wire";

/**
 * Share of the prompt tokens that came from the provider's cache, in whole percent:
 * `cache_read / (input + cache_read + cache_creation)`. The three are the prompt's disjoint parts
 * (uncached, served from cache, written to cache), so the denominator is every prompt token the
 * model was given; tokens written to the cache count as a miss, not a hit.
 */
export function cachePercent(u: Pick<Usage, "input_tokens" | "cache_read_tokens" | "cache_creation_tokens">): number {
  const prompt = u.input_tokens + u.cache_read_tokens + u.cache_creation_tokens;
  return prompt ? Math.round((u.cache_read_tokens / prompt) * 100) : 0;
}

export interface UsageSummary {
  /** The agent reports cost / token counts at all (`capabilities`): otherwise the figures below are zeros, not facts. */
  reportsCost: boolean;
  reportsUsage: boolean;
  cost: number;
  turns: number;
  /** Every prompt token: uncached + read from cache + written to cache. */
  promptTokens: number;
  outputTokens: number;
  cacheWrittenTokens: number;
  cachePct: number;
  /** `null`: the agent does not report it. */
  contextPct: number | null;
  /** Cost and tokens per model, as the snapshot carries them (empty when the agent reports no split). */
  perModel: [string, ModelUsage][];
}

/** Every usage figure of a session in one place: the session sidebar and the header both read it. */
export function usageSummary(snap: SessionSnapshot): UsageSummary {
  // The live report is newer than the last completed turn's totals once it has caught up with them.
  const u = snap.live_usage && snap.live_usage.input_tokens >= snap.usage.input_tokens ? snap.live_usage : snap.usage;
  return {
    reportsCost: snap.capabilities.cost,
    reportsUsage: snap.capabilities.usage,
    cost: u.cost_usd,
    turns: snap.turns_completed,
    promptTokens: u.input_tokens + u.cache_read_tokens + u.cache_creation_tokens,
    outputTokens: u.output_tokens,
    cacheWrittenTokens: u.cache_creation_tokens,
    cachePct: cachePercent(u),
    contextPct: snap.context_pct ?? null,
    perModel: Object.entries(snap.usage_by_model ?? {}),
  };
}
