import { useEffect, useMemo, useRef, type ReactNode } from "react";
import type { DriverDescriptor } from "../../ui";
import { Button, Skeleton } from "../../ui";
import { scopeKey, useApp, useAppState, useTranscriptOpen, type TranscriptScope } from "../../state";
import { TranscriptItem } from "./TranscriptView";
import { TranscriptCtx } from "./transcriptContext";
import { useTranscriptRows } from "./useTranscriptRows";

/**
 * One transcript, scrolling: the session's own conversation or a subagent's (by `scope`). It opens the
 * transcript while mounted (history, then live), shows it with "Load earlier messages", and keeps the bottom in view
 * while it grows (unless the reader scrolled up). `children` follow the last row (a session's pending requests).
 * `empty`: what to say when the transcript has loaded and holds nothing.
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
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const info = useMemo(() => ({ key: sessionKey, host, id }), [sessionKey, host, id]);

  useEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [items.length, followKey]);
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
        <div className="tr">
          {slice?.hasMore ? <Button size="sm" variant="ghost" pending={slice.loading} onClick={() => void transcripts.loadEarlier(sessionKey, scope)}>Load earlier messages</Button> : null}
          {!slice || (slice.loading && items.length === 0) ? <><Skeleton width="50%" /><Skeleton /><Skeleton width="70%" /></> : null}
          {slice?.error ? <div className="c-failed" role="alert">{slice.error}</div> : null}
          {slice && !slice.loading && !slice.error && items.length === 0 && empty ? <div className="faint fs12" data-testid="transcript-empty">{empty}</div> : null}
          {items.map((it) => <TranscriptItem key={it.key} item={it} driver={driver} />)}
          {children}
        </div>
      </TranscriptCtx.Provider>
    </div>
  );
}
