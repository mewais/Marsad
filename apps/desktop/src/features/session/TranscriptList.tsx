import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { EarlierHistory, SearchHit, SearchPart } from "../../bridge/wire";
import type { DriverDescriptor } from "../../ui";
import { Button, Skeleton, VirtualList, type VirtualListHandle } from "../../ui";
import { scopeKey, useApp, useAppState, useTranscriptOpen, type TranscriptScope } from "../../state";
import { cursorIndex, stepIndex } from "../../state/find";
import { useShortcut } from "../../shortcuts/useShortcut";
import { hasEarlier } from "./earlierWords";
import { ExpansionCtx, expansionFor } from "./expansion";
import { EarlierEnd, EarlierTop } from "./EarlierPart";
import { FindBar } from "./FindBar";
import { Outline } from "./Outline";
import { TranscriptItem } from "./TranscriptView";
import { TranscriptCtx } from "./transcriptContext";
import { useFind } from "./useFind";
import { useTranscriptJump, type JumpTarget } from "./useTranscriptJump";
import { useTranscriptRows } from "./useTranscriptRows";
import { useTranscriptItems, type TranscriptEntry } from "./useTranscriptItems";
import { clearMarks } from "./textMarks";

const sameReport = (a: EarlierHistory | undefined, b: EarlierHistory | undefined) => a === b || JSON.stringify(a) === JSON.stringify(b);

/** Height guesses for entries not measured yet: rows are the bulk of a transcript, the rest are short or empty until measured. */
const ESTIMATE: Record<TranscriptEntry["kind"], number> = { head: 40, earlier: 120, "earlier-end": 40, row: 120, tail: 0 };
const estimate = (e: TranscriptEntry) => ESTIMATE[e.kind];
const entryKey = (e: TranscriptEntry) => e.key;

/** The jump a hit of the find (or of the outline, without a query) is. */
const targetOf = (hit: SearchHit, query: string): JumpTarget => ({ part: hit.part, cursor: hit.cursor, query, hint: { context: hit.snippet, start: hit.start } });

/**
 * One transcript, scrolling: the session's own conversation or a subagent's (by `scope`). It opens the
 * transcript while mounted (history, then live), and shows it in one virtual list that starts at the bottom and follows it while
 * it grows, until the reader scrolls up. Reaching the top pages in earlier messages ("Load earlier messages" does the same).
 * `children` follow the last row (a session's pending requests). `empty`: what to say when the transcript has loaded and holds nothing.
 *
 * A resumed conversation's earlier history is the transcript's upper part, above a divider and the session's own rows: the same
 * list, paged with the same control. It is shown once the session's own events are all here (so nothing sits between the two),
 * with what the daemon says about its loading.
 *
 * Above the list: the transcript's header (Find, Outline, Collapse all, Expand all). Find (Ctrl+F) and the outline move the reader
 * to a hit or a prompt through the same jump (`useTranscriptJump`); Alt+Up/Alt+Down move between the prompts of what is loaded.
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

  const listRef = useRef<VirtualListHandle>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const liveIndex = useMemo(() => cursorIndex(items, (r) => r.key), [items]);
  const earlierIndex = useMemo(() => cursorIndex(earlierRows, (r) => `earlier:${r.key}`), [earlierRows]);
  const jumps = useTranscriptJump({
    sessionKey,
    host,
    id,
    scope,
    tk,
    live: { index: liveIndex, win: { oldest: slice?.events[0]?.cursor, hasMore: !!slice?.hasMore, loading: !!slice?.loading } },
    earlier: { index: earlierIndex, win: { oldest: slice?.earlier.events[0]?.cursor, hasMore: !!slice?.earlier.hasMore, loading: !!slice?.earlier.loading } },
    listRef,
    rootRef,
    expansion,
  });
  const { detached, detachedRows, pinned } = jumps;

  const canLoad = !!slice && (slice.hasMore || !!shown?.list.hasMore);
  const loadEarlier = () => void transcripts.loadEarlier(sessionKey, scope);
  // A reach of the top asks for one page, as the button does, and only when there is one to ask for and none in flight.
  const onReachStart = () => {
    if (canLoad && !slice?.loading && !slice?.earlier.loading) loadEarlier();
  };

  // The rows shown: the detached window while the reader is in one, else the transcript's list.
  const detachedEntries: TranscriptEntry[] = [
    { kind: "head", key: "head" },
    ...detachedRows.map((r): TranscriptEntry => (detached?.part === "earlier" ? { kind: "earlier", key: `d:${r.key}`, row: r } : { kind: "row", key: `d:${r.key}`, row: r })),
    { kind: "tail", key: "tail", follow: followKey },
  ];
  const shownEntries = detached ? detachedEntries : entries;

  // The find: the hits stream in (newest first), the active one is the one the reader is on. The first hit is jumped to.
  const [findOpen, setFindOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(-1);
  const parts: SearchPart[] = scope.kind === "main" && hasEarlier(earlier) ? ["live", "earlier"] : ["live"];
  const found = useFind(host, id, scope, parts, findOpen ? query : "");
  const hits = found.hits;
  useEffect(() => {
    if (!findOpen || active >= 0 || hits.length === 0) return;
    setActive(0);
    jumps.jump(targetOf(hits[0], query));
    // eslint-disable-next-line react-hooks/exhaustive-deps -- the first hit of a query is jumped to once, when it arrives.
  }, [hits.length, findOpen]);

  const openFind = () => {
    if (findOpen) {
      (rootRef.current?.querySelector(".tr-find-input") as HTMLInputElement | null)?.focus();
      return;
    }
    setFindOpen(true);
  };
  const closeFind = () => {
    setFindOpen(false);
    setQuery("");
    setActive(-1);
    clearMarks();
  };
  const stepFind = (dir: 1 | -1) => {
    const next = stepIndex(active, hits.length, dir);
    if (next < 0) return;
    setActive(next);
    jumps.jump(targetOf(hits[next], query));
  };

  const [outlineOpen, setOutlineOpen] = useState(false);

  // Alt+Up / Alt+Down: the prompts of what is shown, from the last one the reader moved to (the bottom, before any move).
  const movePrompt = (dir: 1 | -1) => {
    const prompts: { key: string; at: number }[] = [];
    shownEntries.forEach((e, at) => {
      if ((e.kind === "row" || e.kind === "earlier") && e.row.kind === "user") prompts.push({ key: e.key, at });
    });
    const cur = jumps.navRef.current;
    const at = cur == null ? -1 : shownEntries.findIndex((e) => e.key === cur);
    const anchor = at < 0 ? (dir < 0 ? Number.POSITIVE_INFINITY : Number.NEGATIVE_INFINITY) : at;
    const target = dir < 0 ? [...prompts].reverse().find((p) => p.at < anchor) : prompts.find((p) => p.at > anchor);
    if (!target) return;
    jumps.navRef.current = target.key;
    jumps.unpin();
    listRef.current?.scrollToKey(target.key);
  };

  useShortcut({
    id: "transcript-find",
    keys: ["mod+f"],
    title: "Find in the transcript",
    group: "Session",
    allowInInput: true,
    // With the transcript focused (or nothing focused); Ctrl+F in another field keeps its own meaning.
    when: () => {
      const a = document.activeElement;
      return !a || a === document.body || !!rootRef.current?.contains(a);
    },
    run: openFind,
  });
  useShortcut({ id: "transcript-prev-prompt", keys: ["alt+arrowup"], title: "Previous prompt", group: "Session", run: () => movePrompt(-1) });
  useShortcut({ id: "transcript-next-prompt", keys: ["alt+arrowdown"], title: "Next prompt", group: "Session", run: () => movePrompt(1) });

  const renderEntry = (e: TranscriptEntry) => {
    switch (e.kind) {
      case "head":
        if (detached) {
          return (
            <div className="tr-item tr-head">
              <div className="tr-band" role="status">
                <span>Viewing older part</span>
                <span aria-hidden="true"> · </span>
                <Button size="sm" variant="ghost" onClick={jumps.backToLatest}>Back to latest</Button>
                {detached.error ? <span className="c-failed"> {detached.error}</span> : null}
              </div>
            </div>
          );
        }
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
        if (detached) return <div className="tr-item tr-tail" />;
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
        <div className="tr-col" ref={rootRef}>
          <div className="tr-bar" role="group" aria-label="Transcript">
            {!pinned && !detached ? <Button size="sm" variant="ghost" onClick={jumps.backToLatest}>Back to latest</Button> : null}
            <span className="tr-spacer" />
            <Button size="sm" variant="ghost" icon="search" title="Find in the transcript (Ctrl+F)" onClick={openFind}>Find</Button>
            <Button size="sm" variant="ghost" aria-expanded={outlineOpen} onClick={() => setOutlineOpen((o) => !o)}>Outline</Button>
            <Button size="sm" variant="ghost" onClick={() => expansion.setAll(false)}>Collapse all</Button>
            <Button size="sm" variant="ghost" onClick={() => expansion.setAll(true)}>Expand all</Button>
          </div>
          {findOpen ? (
            <FindBar
              query={query}
              onQuery={(q) => {
                setQuery(q);
                setActive(-1);
              }}
              count={hits.length}
              active={active}
              searching={!found.done}
              error={found.error}
              onStep={stepFind}
              onClose={closeFind}
            />
          ) : null}
          <div className="tr-stage">
            <VirtualList<TranscriptEntry>
              ref={listRef}
              className="transcript"
              items={shownEntries}
              getKey={entryKey}
              estimateSize={estimate}
              renderItem={renderEntry}
              anchor="end"
              initialCount={60}
              stickToEnd={pinned && !detached}
              keepAnchor
              onReachStart={detached ? undefined : onReachStart}
            />
            {outlineOpen ? (
              <Outline
                host={host}
                id={id}
                scope={scope}
                parts={parts}
                onJump={(h) => jumps.jump(targetOf(h, ""))}
                onClose={() => setOutlineOpen(false)}
              />
            ) : null}
          </div>
        </div>
      </ExpansionCtx.Provider>
    </TranscriptCtx.Provider>
  );
}
