import { describe, expect, it } from "vitest";
import type { EarlierGaps, EarlierHistory, EarlierUnavailable } from "../../src/bridge/wire";
import { emptyGaps } from "../../src/bridge/fixture/earlier";
import { earlierNotes, gapWords, hasEarlier, unavailableWords } from "../../src/features/session/earlierWords";

const gaps = (g: Partial<EarlierGaps> = {}): EarlierGaps => ({ ...emptyGaps(), ...g });
const earlier = (state: EarlierHistory["state"], over: Partial<EarlierHistory> = {}): EarlierHistory => ({ state, items: 0, gaps: gaps(), ...over });

describe("what the earlier history leaves out, in words", () => {
  it("says nothing when nothing is left out", () => {
    expect(gapWords(gaps())).toEqual([]);
    // Replays the agent sent again and the runtime already had are not a gap: nothing is missing.
    expect(gapWords(gaps({ replays_ignored: 3 }))).toEqual([]);
  });

  it.each<[string, Partial<EarlierGaps>, string]>([
    ["older messages", { omitted: 120 }, "120 older messages exist in the agent's own history and are not shown."],
    ["one older message", { omitted: 1 }, "1 older message exists in the agent's own history and is not shown."],
    ["shortened items", { shortened: 2 }, "2 items were too large to keep in full and are shortened, or have images or files left out."],
    ["unreadable lines", { unreadable: 4 }, "4 lines of the agent's file could not be read and are missing."],
    ["records of unknown kinds", { unrecognised: 3, unrecognised_kinds: ["queue-operation", "summary"] }, "3 records were not understood (queue-operation, summary) and are missing."],
    ["every kind named", { unrecognised: 1, unrecognised_kinds: ["summary"] }, "1 record was not understood (summary) and is missing."],
    ["records of unknown kinds without names", { unrecognised: 2 }, "2 records were not understood and are missing."],
    ["files that could not be read", { missing_files: 2 }, "2 files of the conversation (a subagent's own transcript) could not be read, so those parts are missing."],
    ["one file", { missing_files: 1 }, "1 file of the conversation (a subagent's own transcript) could not be read, so that part is missing."],
    ["a cut last line", { truncated_tail: true }, "The last line of the agent's file was cut off while the agent was writing it, so the very last message may be missing."],
  ])("%s", (_name, g, words) => {
    expect(gapWords(gaps(g))).toEqual([words]);
  });

  it("several gaps are several sentences, in a fixed order", () => {
    expect(gapWords(gaps({ truncated_tail: true, omitted: 5, shortened: 1 }))).toHaveLength(3);
    expect(gapWords(gaps({ truncated_tail: true, omitted: 5, shortened: 1 }))[0]).toMatch(/^5 older messages/);
  });
});

describe("why the earlier conversation cannot be shown, in words", () => {
  it.each<[EarlierUnavailable, RegExp]>([
    [{ kind: "not_offered" }, /^This agent does not give out its earlier conversation, so it cannot be shown here\.$/],
    [{ kind: "source_missing" }, /^The agent's file for this conversation could not be found, so the earlier conversation cannot be shown\.$/],
    [{ kind: "source_unreadable", detail: "permission denied" }, /could not be read \(permission denied\), so the earlier conversation cannot be shown\.$/],
    [{ kind: "too_large", bytes: 3 * 1024 ** 3, limit: 1024 ** 3 }, /is too large to read \(3\.0 GB; the limit is 1\.0 GB\), so the earlier conversation cannot be shown\.$/],
    [{ kind: "format_not_understood", record: 4 }, /^Marsad does not understand the agent's file for this conversation \(the first line it could not read is number 4\), so/],
    [{ kind: "failed", detail: "out of memory" }, /^Loading the earlier conversation failed \(out of memory\)\.$/],
  ])("%j", (reason, words) => {
    expect(unavailableWords(reason)).toMatch(words);
  });

  it("a reason this build does not know is said generically, never refused", () => {
    expect(unavailableWords({ kind: "a_future_reason" } as unknown as EarlierUnavailable)).toBe("The earlier conversation could not be loaded.");
  });
});

describe("the notices of an earlier history", () => {
  it("none: nothing, and there is no earlier part at all", () => {
    expect(earlierNotes(earlier({ kind: "none" }))).toEqual({ top: [], joint: null });
    expect(hasEarlier(earlier({ kind: "none" }))).toBe(false);
    expect(hasEarlier(undefined)).toBe(false);
  });

  it("loading: says so where the rest will appear, with the count so far", () => {
    expect(earlierNotes(earlier({ kind: "loading" })).joint).toEqual({ tone: "loading", text: "Loading the earlier conversation…" });
    expect(earlierNotes(earlier({ kind: "loading" }, { items: 40 })).joint).toEqual({ tone: "loading", text: "Loading the earlier conversation… 40 items so far" });
    expect(earlierNotes(earlier({ kind: "loading" }, { items: 1 })).joint?.text).toBe("Loading the earlier conversation… 1 item so far");
    expect(hasEarlier(earlier({ kind: "loading" }))).toBe(true);
  });

  it("loaded complete: nothing to say; loaded with gaps: the gaps at the top", () => {
    expect(earlierNotes(earlier({ kind: "loaded", complete: true }, { items: 9 }))).toEqual({ top: [], joint: null });
    const n = earlierNotes(earlier({ kind: "loaded", complete: false }, { items: 9, gaps: gaps({ omitted: 7 }) }));
    expect(n.top).toEqual(["7 older messages exist in the agent's own history and are not shown."]);
    expect(n.joint).toBeNull();
  });

  it("a process that ended before it all arrived says the newest part is missing, where it is missing", () => {
    const n = earlierNotes(earlier({ kind: "loaded", complete: false }, { items: 9, gaps: gaps({ interrupted: true }) }));
    expect(n.top).toEqual([]);
    expect(n.joint).toMatchObject({ tone: "interrupted" });
    expect(n.joint?.text).toMatch(/newest part is missing here/);
  });

  it("unavailable: the reason in words, and what arrived before it stopped", () => {
    expect(earlierNotes(earlier({ kind: "unavailable", reason: { kind: "source_missing" } })).joint).toEqual({ tone: "unavailable", text: "The agent's file for this conversation could not be found, so the earlier conversation cannot be shown." });
    const partial = earlierNotes(earlier({ kind: "unavailable", reason: { kind: "failed", detail: "disk" } }, { items: 12, gaps: gaps({ unreadable: 1 }) }));
    expect(partial.joint?.text).toMatch(/failed \(disk\)\. 12 items arrived before that and are shown\.$/);
    expect(partial.top).toHaveLength(1);
  });

  it("a state this build does not know is said, not hidden", () => {
    expect(earlierNotes({ state: { kind: "from_the_future" }, items: 0, gaps: gaps() } as unknown as EarlierHistory).joint?.tone).toBe("unavailable");
  });
});
