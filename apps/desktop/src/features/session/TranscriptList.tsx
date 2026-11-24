import { useEffect, useMemo, useRef, type ReactNode } from "react";
import type { EarlierHistory } from "../../bridge/wire";
import type { DriverDescriptor } from "../../ui";
import { Button, Skeleton } from "../../ui";
import { scopeKey, useApp, useAppState, useTranscriptOpen, type TranscriptScope } from "../../state";
import { hasEarlier } from "./earlierWords";
import { ExpansionCtx, expansionFor } from "./expansion";
import { EarlierPart } from "./EarlierPart";
import { TranscriptItem } from "./TranscriptView";
import { TranscriptCtx } from "./transcriptContext";
import { useTranscriptRows } from "./useTranscriptRows";

const sameReport = (a: EarlierHistory | undefined, b: EarlierHistory | undefined) => a === b || JSON.stringify(a) === JSON.stringify(b);

/**
 * One transcript, scrolling: the session's own conversation or a subagent's (by `scope`). It opens the
 * transcript while mounted (history, then live), shows it with "Load earlier messages", and keeps the bottom in view
 * while it grows (unless the reader scrolled up). `children` follow the last row (a session's pending requests).
 * `empty`: what to say when the transcript has loaded and holds nothing.
 *
 * A resumed conversation's earlier history is the transcript's upper part (`EarlierPart`), above a divider and the session's
 * own rows: two lists, shown one after the other, paged with the same control. It is shown once the session's own events are all
 * here (so nothing sits between the two), with what the daemon says about its loading.
 */
export function TranscriptList({ sessionKey, host, id, scope, driver, followKey, empty, children }: {
  sessionKey: string;
  host: string;
  id: string;
  scope: TranscriptScope;
  driver?: DriverDescriptor;
  /** Changes when something that follows the rows appears, so the view scrolls to it. */
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
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const info = useMemo(() => ({ key: sessionKey, host, id }), [sessionKey, host, id]);
  // What is expanded belongs to the transcript, not to the rows: it survives a row (or this list) being unmounted.
  const expansion = useMemo(() => expansionFor(tk), [tk]);

  useEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [items.length, earlierRows.length, followKey]);
  // Images load after the items render: keep the bottom in view while it grows.
  useEffect(() => {
    const el = scroller.current;
    const inner = el?.firstElementChild;
    if (!el || !inner || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      if (stick.current) el.scrollTop = el.scrollHeight;
    });
    ro.observe(inner);
    return () => ro.disconnect();
  }, [slice !== undefined]);

  return (
    <div className="transcript" ref={scroller} onScroll={(e) => { const el = e.currentTarget; stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80; }}>
      <TranscriptCtx.Provider value={info}>
        <ExpansionCtx.Provider value={expansion}>
          <div className="tr">
            {slice && (slice.hasMore || shown?.list.hasMore) ? <Button size="sm" variant="ghost" pending={slice.loading || slice.earlier.loading} onClick={() => void transcripts.loadEarlier(sessionKey, scope)}>Load earlier messages</Button> : null}
            {!slice || (slice.loading && items.length === 0) ? <><Skeleton width="50%" /><Skeleton /><Skeleton width="70%" /></> : null}
            {slice?.error ? <div className="c-failed" role="alert">{slice.error}</div> : null}
            {shown ? <EarlierPart earlier={shown.info} rows={earlierRows} driver={driver} error={shown.list.error} retrying={shown.list.loading} onRetry={() => void transcripts.loadEarlier(sessionKey, scope)} /> : null}
            {slice && !slice.loading && !slice.error && items.length === 0 && empty ? <div className="faint fs12" data-testid="transcript-empty">{empty}</div> : null}
            {items.map((it) => <TranscriptItem key={it.key} item={it} driver={driver} />)}
            {children}
          </div>
        </ExpansionCtx.Provider>
      </TranscriptCtx.Provider>
    </div>
  );
}
