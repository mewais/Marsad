import { useMemo, type ReactNode } from "react";
import type { EarlierHistory } from "../../bridge/wire";
import type { DriverDescriptor } from "../../ui";
import { Button, Skeleton, VirtualList } from "../../ui";
import { scopeKey, useApp, useAppState, useTranscriptOpen, type TranscriptScope } from "../../state";
import { hasEarlier } from "./earlierWords";
import { ExpansionCtx, expansionFor } from "./expansion";
import { EarlierEnd, EarlierTop } from "./EarlierPart";
import { TranscriptItem } from "./TranscriptView";
import { TranscriptCtx } from "./transcriptContext";
import { useTranscriptRows } from "./useTranscriptRows";
import { useTranscriptItems, type TranscriptEntry } from "./useTranscriptItems";

const sameReport = (a: EarlierHistory | undefined, b: EarlierHistory | undefined) => a === b || JSON.stringify(a) === JSON.stringify(b);

/** Height guesses for entries not measured yet: rows are the bulk of a transcript, the rest are short or empty until measured. */
const ESTIMATE: Record<TranscriptEntry["kind"], number> = { head: 40, earlier: 120, "earlier-end": 40, row: 120, tail: 0 };
const estimate = (e: TranscriptEntry) => ESTIMATE[e.kind];
const entryKey = (e: TranscriptEntry) => e.key;

/**
 * One transcript, scrolling: the session's own conversation or a subagent's (by `scope`). It opens the
 * transcript while mounted (history, then live), and shows it in one virtual list that starts at the bottom and follows it while
 * it grows, until the reader scrolls up. Reaching the top pages in earlier messages ("Load earlier messages" does the same).
 * `children` follow the last row (a session's pending requests). `empty`: what to say when the transcript has loaded and holds nothing.
 *
 * A resumed conversation's earlier history is the transcript's upper part, above a divider and the session's own rows: the same
 * list, paged with the same control. It is shown once the session's own events are all here (so nothing sits between the two),
 * with what the daemon says about its loading.
 */
export function TranscriptList({ sessionKey, host, id, scope, driver, followKey, empty, children }: {
  sessionKey: string;
  host: string;
  id: string;
  scope: TranscriptScope;
  driver?: DriverDescriptor;
  /** Changes when something that follows the rows appears, so the view scrolls to it (the tail is re-laid out). */
  followKey?: number;
  empty?: ReactNode;
  children?: ReactNode;
}) {
  const { transcripts } = useApp();
  useTranscriptOpen(sessionKey, scope);
  const tk = scopeKey(sessionKey, scope);
  const slice = useAppState((s) => s.transcripts[tk]);
  const items = useTranscriptRows(tk, slice?.events, scope);
  // What the daemon reports of the conversation's earlier history; a new snapshot with the same report is not a change.
  const earlier = useAppState((s) => s.sessions[sessionKey]?.earlier, sameReport);
  const earlierRows = useTranscriptRows(tk, slice?.earlier.events, scope, "earlier");
  const shown = slice && !slice.hasMore && hasEarlier(earlier) ? { info: earlier, list: slice.earlier } : null;
  const info = useMemo(() => ({ key: sessionKey, host, id }), [sessionKey, host, id]);
  // What is expanded belongs to the transcript, not to the rows: it survives a row (or this list) being unmounted.
  const expansion = useMemo(() => expansionFor(tk), [tk]);
  const entries = useTranscriptItems(earlierRows, items, shown !== null, followKey);

  const canLoad = !!slice && (slice.hasMore || !!shown?.list.hasMore);
  const loadEarlier = () => void transcripts.loadEarlier(sessionKey, scope);
  // A reach of the top asks for one page, as the button does, and only when there is one to ask for and none in flight.
  const onReachStart = () => {
    if (canLoad && !slice?.loading && !slice?.earlier.loading) loadEarlier();
  };

  const renderEntry = (e: TranscriptEntry) => {
    switch (e.kind) {
      case "head":
        return (
          <div className="tr-item tr-head">
            {canLoad ? <Button size="sm" variant="ghost" pending={!!slice?.loading || !!slice?.earlier.loading} onClick={loadEarlier}>Load earlier messages</Button> : null}
            {!slice || (slice.loading && items.length === 0) ? <><Skeleton width="50%" /><Skeleton /><Skeleton width="70%" /></> : null}
            {slice?.error ? <div className="c-failed" role="alert">{slice.error}</div> : null}
            {shown ? <EarlierTop earlier={shown.info} error={shown.list.error} retrying={shown.list.loading} onRetry={loadEarlier} /> : null}
          </div>
        );
      case "earlier":
        return <div className="tr-item hist-part" data-part="earlier"><TranscriptItem item={e.row} driver={driver} part="earlier" scope={scope} /></div>;
      case "earlier-end":
        return <div className="tr-item">{shown ? <EarlierEnd earlier={shown.info} /> : null}</div>;
      case "row":
        return <div className="tr-item" data-part="live"><TranscriptItem item={e.row} driver={driver} part="live" scope={scope} /></div>;
      case "tail":
        return (
          <div className="tr-item tr-tail">
            {slice && !slice.loading && !slice.error && items.length === 0 && empty ? <div className="faint fs12" data-testid="transcript-empty">{empty}</div> : null}
            {children}
          </div>
        );
    }
  };

  return (
    <TranscriptCtx.Provider value={info}>
      <ExpansionCtx.Provider value={expansion}>
        <VirtualList<TranscriptEntry>
          className="transcript"
          items={entries}
          getKey={entryKey}
          estimateSize={estimate}
          renderItem={renderEntry}
          anchor="end"
          initialCount={60}
          stickToEnd
          keepAnchor
          onReachStart={onReachStart}
        />
      </ExpansionCtx.Provider>
    </TranscriptCtx.Provider>
  );
}
