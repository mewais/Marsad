import { useMemo, useRef } from "react";
import type { SessionEvent } from "../../bridge/wire";
import { MAIN, type TranscriptScope } from "../../state/scope";
import { groupToolRuns, type RowItem } from "./toolRuns";
import { createTranscriptFold } from "./transcript";

const NONE: readonly SessionEvent[] = [];

/**
 * The rows of a transcript (the session's own or a subagent's, by `scope`). The fold and the grouping both keep the identity of whatever the
 * new events did not touch, so a live event re-renders only the rows it changed.
 */
export function useTranscriptRows(sessionKey: string, events: readonly SessionEvent[] | undefined, scope: TranscriptScope = MAIN): readonly RowItem[] {
  const callId = scope.kind === "subagent" ? scope.call_id : null;
  const fold = useMemo(() => createTranscriptFold(callId === null ? MAIN : { kind: "subagent", call_id: callId }), [sessionKey, callId]);
  const last = useRef<readonly RowItem[]>([]);
  return useMemo(() => {
    const rows = groupToolRuns(fold.update(events ?? NONE), last.current);
    if (rows.length === last.current.length && rows.every((r, i) => r === last.current[i])) return last.current;
    last.current = rows;
    return rows;
  }, [fold, events]);
}
