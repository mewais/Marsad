import { describe, expect, it } from "vitest";
import type { CompactionInfo } from "../../src/bridge/wire";
import { compactionWords } from "../../src/features/session/transcript";

// Local time, so the clock words are the same wherever the suite runs.
const AT = new Date(2026, 10, 24, 14, 2).getTime();
const FULL: CompactionInfo = { trigger: "manual", pre_tokens: 120_000, post_tokens: 40_000, duration_ms: 2100, cumulative_dropped_tokens: 80_000 };

describe("compactionWords: a compaction says what the CLI reported, and nothing it did not", () => {
  it("with only the trigger known, says that and when", () => {
    expect(compactionWords({ trigger: "manual" }, AT)).toBe("History compacted at 14:02 (manual)");
  });

  it("with nothing known, says it happened and no more", () => {
    expect(compactionWords({}, undefined)).toBe("History compacted");
  });

  it("with every part known, gives the tokens, the duration and the dropped count", () => {
    expect(compactionWords(FULL, AT)).toBe("History compacted at 14:02 (manual): 120k → 40k tokens in 2.1s · 80k dropped so far");
  });

  it("gives the tokens before alone, or after alone, when only one side is known", () => {
    expect(compactionWords({ trigger: null, pre_tokens: 120_000 }, AT)).toBe("History compacted at 14:02: 120k tokens before");
    expect(compactionWords({ trigger: "auto", post_tokens: 40_000 }, undefined)).toBe("History compacted (auto): 40k tokens after");
  });

  it("gives the duration alone and the dropped count alone", () => {
    expect(compactionWords({ trigger: "manual", duration_ms: 2100 }, AT)).toBe("History compacted at 14:02 (manual): in 2.1s");
    expect(compactionWords({ trigger: "manual", cumulative_dropped_tokens: 80_000 }, AT)).toBe("History compacted at 14:02 (manual) · 80k dropped so far");
  });

  it("never prints an absent part as 0, null, undefined or an empty clause, whichever part is missing", () => {
    const parts = Object.keys(FULL) as (keyof CompactionInfo)[];
    for (const missing of parts) {
      const c: CompactionInfo = { ...FULL };
      delete c[missing];
      const words = compactionWords(c, AT);
      expect(words, `without ${missing}`).not.toMatch(/undefined|null|NaN|\(\)|: *$|: ·|·\s*·|\s{2,}|\b0(\.0)?(s|k| tokens| dropped)\b/);
      expect(words, `without ${missing}`).toMatch(/^History compacted/);
    }
    expect(compactionWords({}, undefined)).not.toMatch(/0|null|undefined/);
  });
});
