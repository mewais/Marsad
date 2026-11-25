import { useEffect, useMemo, useState } from "react";
import { errorMessage } from "../../bridge/api";
import type { SessionEvent } from "../../bridge/wire";
import { copyText } from "../../lib/clipboard";
import { Button, Dialog, toast } from "../../ui";
import { useApi, useAppState } from "../../state";
import { admits, scopeKey, type TranscriptScope } from "../../state/scope";
import type { Part, Span } from "./transcript";

/** What the drawer shows at most; "Copy JSON" copies the whole of it. */
export const RAW_CAP_BYTES = 200 * 1024;
/** Pages read back from the host for one item's span at most (the span is one item: a few pages at most in practice). */
const MAX_PAGES = 20;
const PAGE = 500;

/** Events in cursor order, each cursor once: what was read and what was already held. */
function mergeByCursor(...lists: readonly SessionEvent[][]): SessionEvent[] {
  const all = lists.flat().sort((a, b) => a.cursor - b.cursor);
  return all.filter((e, i) => i === 0 || e.cursor !== all[i - 1].cursor);
}

/**
 * The events of one item: those in its cursor span that its transcript admits (its scope, and for the earlier history the agent's
 * events only), in cursor order.
 */
export function spanEvents(events: readonly SessionEvent[], span: Span, scope: TranscriptScope, part: Part): SessionEvent[] {
  return events.filter((e) => e.cursor >= span.from && e.cursor <= span.to && admits(scope, e) && (part === "live" || e.kind === "agent"));
}

/** The pretty JSON of `events` and whether `cap` cut it short. */
export function rawJson(events: readonly SessionEvent[], cap = RAW_CAP_BYTES): { text: string; full: string; truncated: boolean } {
  const full = JSON.stringify(events, null, 2);
  const truncated = full.length > cap;
  return { text: truncated ? full.slice(0, cap) : full, full, truncated };
}

export interface RawEventDrawerProps {
  sessionKey: string;
  host: string;
  id: string;
  scope: TranscriptScope;
  part: Part;
  span: Span;
  onClose: () => void;
}

/**
 * The raw events behind one transcript item, as pretty JSON. Events the transcript has already loaded are used as they are; when the
 * item starts before what is loaded, the earlier events are read from the host (read here only: the transcript itself is not changed).
 */
export function RawEventDrawer({ sessionKey, host, id, scope, part, span, onClose }: RawEventDrawerProps) {
  const api = useApi();
  const tk = scopeKey(sessionKey, scope);
  const slice = useAppState((s) => s.transcripts[tk]);
  const list = part === "earlier" ? slice?.earlier : slice;
  const loaded = list?.events ?? [];
  const hasMore = !!list?.hasMore;
  const [fetched, setFetched] = useState<{ events: SessionEvent[]; loading: boolean; error?: string }>({ events: [], loading: false });
  const firstLoaded = loaded[0]?.cursor ?? span.to + 1;
  const needsFetch = hasMore && span.from < firstLoaded;

  useEffect(() => {
    if (!needsFetch) return;
    let live = true;
    setFetched({ events: [], loading: true });
    void readBefore(api, host, id, scope, part === "earlier", span, firstLoaded).then(
      (events) => live && setFetched({ events, loading: false }),
      (e: unknown) => live && setFetched({ events: [], loading: false, error: errorMessage(e) }),
    );
    return () => {
      live = false;
    };
    // The span and the scope identify the read; the loaded events only decide whether it is needed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [needsFetch, host, id, part, span.from, span.to, tk]);

  const events = useMemo(() => spanEvents(mergeByCursor(fetched.events, loaded), span, scope, part), [fetched.events, loaded, span, scope, part]);
  const json = useMemo(() => rawJson(events), [events]);
  const copy = () =>
    copyText(json.full).then(
      () => toast({ tone: "ok", title: "Copied the raw events", ttl: 2000 }),
      (e: unknown) => toast({ tone: "err", title: "Could not copy", detail: errorMessage(e), ttl: 6000 }),
    );

  return (
    <Dialog open onClose={onClose} title="Raw events" subtitle={`cursors ${span.from}–${span.to}`} width={760} className="raw-drawer"
      footer={<Button variant="primary" size="sm" onClick={copy} disabled={!events.length}>Copy JSON</Button>}>
      {fetched.error ? <div className="c-failed fs12" role="alert">Could not read the earlier events: {fetched.error}</div> : null}
      {fetched.loading ? <div className="faint fs12" role="status">Reading the earlier events…</div> : null}
      {!fetched.loading && !events.length ? <div className="faint fs12" data-testid="raw-empty">No events in this item.</div> : null}
      {events.length ? (
        <>
          {json.truncated ? <div className="faint fs12" data-testid="raw-truncated">Showing the first {RAW_CAP_BYTES / 1024} KB of {Math.ceil(json.full.length / 1024)} KB. Copy JSON copies all of it.</div> : null}
          <pre className="raw-json" data-testid="raw-json">{json.text}</pre>
        </>
      ) : null}
    </Dialog>
  );
}

/**
 * The events of `span` that are before `firstLoaded` and not yet read: pages back from the item's end until the item's start is
 * reached (or the host has no more). Returned in cursor order.
 */
async function readBefore(api: ReturnType<typeof useApi>, host: string, id: string, scope: TranscriptScope, earlier: boolean, span: Span, firstLoaded: number): Promise<SessionEvent[]> {
  let got: SessionEvent[] = [];
  let before = Math.min(firstLoaded, span.to + 1);
  for (let page = 0; page < MAX_PAGES; page++) {
    const r = await api.history(host, { id, before, limit: PAGE, scope, earlier: earlier || undefined });
    got = mergeByCursor(r.events, got);
    const oldest = r.events[0]?.cursor;
    if (!r.has_more || oldest === undefined || oldest <= span.from) break;
    before = oldest;
  }
  return got;
}
