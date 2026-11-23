import { useMemo, useRef } from "react";
import type { SessionEvent } from "../../bridge/wire";
import { groupToolRuns, type RowItem } from "./toolRuns";
import { createTranscriptFold } from "./transcript";

const NONE: readonly SessionEvent[] = [];

/**
 * The rows of a session's transcript. The fold and the grouping both keep the identity of whatever the
 * new events did not touch, so a live event re-renders only the rows it changed.
 */
export function useTranscriptRows(sessionKey: string, events: readonly SessionEvent[] | undefined): readonly RowItem[] {
  const fold = useMemo(() => createTranscriptFold(), [sessionKey]);
  const last = useRef<readonly RowItem[]>([]);
  return useMemo(() => {
    const rows = groupToolRuns(fold.update(events ?? NONE), last.current);
    if (rows.length === last.current.length && rows.every((r, i) => r === last.current[i])) return last.current;
    last.current = rows;
    return rows;
  }, [fold, events]);
}
