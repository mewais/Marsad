import { useMemo } from "react";
import type { RowItem } from "./toolRuns";

/**
 * One entry of a transcript's single list, top to bottom: the head (load, skeleton, errors, the earlier history's top), the earlier
 * rows, the earlier history's end (its notes and the divider), the session's own rows, and the tail (empty note and what follows
 * the rows). Each keeps a stable key, so a row that stays keeps its mounted state (and its expansion, which lives outside it).
 */
export type TranscriptEntry =
  | { kind: "head"; key: "head" }
  | { kind: "earlier"; key: string; row: RowItem }
  | { kind: "earlier-end"; key: "earlier-end" }
  | { kind: "row"; key: string; row: RowItem }
  | { kind: "tail"; key: "tail"; follow?: number };

/**
 * The entries of a transcript. The earlier rows are keyed apart from the session's own, since the two are separate event
 * streams and a row key is only unique within one of them. `showEarlier` adds the earlier history's end (once its top is shown).
 */
export function useTranscriptItems(earlierRows: readonly RowItem[], rows: readonly RowItem[], showEarlier: boolean, followKey?: number): TranscriptEntry[] {
  return useMemo(() => {
    const out: TranscriptEntry[] = [{ kind: "head", key: "head" }];
    for (const row of earlierRows) out.push({ kind: "earlier", key: `earlier:${row.key}`, row });
    if (showEarlier) out.push({ kind: "earlier-end", key: "earlier-end" });
    for (const row of rows) out.push({ kind: "row", key: row.key, row });
    out.push({ kind: "tail", key: "tail", follow: followKey });
    return out;
  }, [earlierRows, rows, showEarlier, followKey]);
}
