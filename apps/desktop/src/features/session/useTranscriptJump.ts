import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type RefObject } from "react";
import { errorMessage } from "../../bridge/api";
import type { SearchPart, SessionEvent } from "../../bridge/wire";
import { useApp, useApi } from "../../state";
import type { TranscriptSlice } from "../../state/model";
import { cursorIndex, expansionIdOf, isMarkdownRow, pagingLeft, planJump, type CursorIndex, type Extent, type LoadedWindow } from "../../state/find";
import type { TranscriptScope } from "../../state/scope";
import type { VirtualListHandle } from "../../ui";
import type { Expansion } from "./expansion";
import { markText } from "./textMarks";
import { useTranscriptRows } from "./useTranscriptRows";
import type { RowItem } from "./toolRuns";

/** Events in a detached window: the hit and the events before it, read from the host in one page. */
const DETACHED_LIMIT = 80;

export interface JumpSource {
  index: CursorIndex;
  win: LoadedWindow;
}

export interface JumpTarget {
  part: SearchPart;
  cursor: number;
  /** The find's query: marked in the row's text. Empty: the row is only brought into view (an outline entry). */
  query: string;
  /** The hit's snippet, to tell which match of the row is the hit. */
  hint?: { context: string; start: number };
}

/** A read-only window of older events around a hit, shown in place of the transcript until the reader goes back to the latest. */
export interface Detached {
  part: SearchPart;
  cursor: number;
  /** Null while the window is read from the host. */
  events: SessionEvent[] | null;
  error?: string;
}

export interface TranscriptJumpArgs {
  sessionKey: string;
  host: string;
  id: string;
  scope: TranscriptScope;
  /** The transcript's store key (the scope's own). */
  tk: string;
  live: JumpSource;
  earlier: JumpSource;
  listRef: RefObject<VirtualListHandle>;
  rootRef: RefObject<HTMLElement>;
  expansion: Expansion;
}

/** The events a store slice gained at the head since `before` (paging only ever prepends older events). */
function grownBy(before: TranscriptSlice | undefined, after: TranscriptSlice | undefined): { events: number; bytes: number } {
  let events = 0;
  let bytes = 0;
  const lists: [readonly SessionEvent[], readonly SessionEvent[]][] = [
    [before?.events ?? [], after?.events ?? []],
    [before?.earlier.events ?? [], after?.earlier.events ?? []],
  ];
  for (const [b, a] of lists) {
    const n = a.length - b.length;
    if (n <= 0) continue;
    for (const e of a.slice(0, n)) {
      events++;
      bytes += JSON.stringify(e).length;
    }
  }
  return { events, bytes };
}

const rowElement = (root: Element | null, key: string): HTMLElement | undefined =>
  root ? ([...root.querySelectorAll<HTMLElement>("[data-vkey]")].find((el) => el.dataset.vkey === key)) : undefined;

/**
 * How a transcript is moved to a hit (B6d): a loaded row is brought into view (its expansion opened, its text marked, or its row
 * flashed when it is markdown); a hit older than the window is paged back to, one page at a time, while the paging budget holds; past
 * the budget, or with nothing older on the host, the hit is read as a detached window around it. The jump is resolved against what is
 * loaded on each change, so it never waits on a timer.
 */
export function useTranscriptJump(a: TranscriptJumpArgs) {
  const { store, transcripts } = useApp();
  const api = useApi();
  const [req, setReq] = useState<JumpTarget | null>(null);
  const [detached, setDetached] = useState<Detached | null>(null);
  const [busy, setBusy] = useState(false);
  const [pinned, setPinned] = useState(true);
  const [hl, setHl] = useState<{ key: string; query: string; hint?: JumpTarget["hint"] } | null>(null);
  const budget = useRef({ events: 0, bytes: 0 });
  const navRef = useRef<string | null>(null);
  const toEnd = useRef(false);

  const detachedRows = useTranscriptRows(`${a.tk}#detached`, detached?.events ?? undefined, a.scope, detached?.part ?? "live");
  const detIndex = useMemo(() => cursorIndex(detachedRows, (r: RowItem) => `d:${r.key}`), [detachedRows]);

  const jump = useCallback((t: JumpTarget) => {
    budget.current = { events: 0, bytes: 0 };
    setDetached(null);
    setPinned(false);
    setHl(null);
    setReq(t);
  }, []);

  const backToLatest = useCallback(() => {
    setDetached(null);
    setPinned(true);
    setReq(null);
    setHl(null);
    toEnd.current = true;
  }, []);

  /** Leaves the latest: the list stops following its end (an explicit move by the reader). */
  const unpin = useCallback(() => setPinned(false), []);

  // Once back on the latest, the list is brought to its end (after the commit that shows the live rows again).
  useLayoutEffect(() => {
    if (toEnd.current && detached === null) {
      toEnd.current = false;
      a.listRef.current?.scrollToEnd();
    }
  });

  const finish = (ext: Extent, r: JumpTarget) => {
    const open = expansionIdOf(ext.row);
    if (open) a.expansion.set(open, true);
    const list = a.listRef.current;
    if (!r.query) list?.scrollToKey(ext.key);
    else if (isMarkdownRow(ext.row)) list?.scrollToKey(ext.key, { highlight: true });
    else {
      list?.scrollToKey(ext.key);
      setHl({ key: ext.key, query: r.query, hint: r.hint });
    }
    navRef.current = ext.key;
    setReq(null);
  };

  // The resolution: what the target is now, given what is loaded. It runs on every change of what is held or wanted.
  useLayoutEffect(() => {
    if (!req || busy || a.live.win.loading || a.earlier.win.loading) return;
    if (detached) {
      if (!detached.events) return;
      const ext = detIndex.at(req.cursor) ?? detIndex.extents[0];
      if (ext) finish(ext, req);
      else setReq(null);
      return;
    }
    const src = req.part === "earlier" ? a.earlier : a.live;
    const plan = planJump(req.cursor, src.index, src.win);
    if (plan === "wait") return;
    if (plan.kind === "row") {
      finish(plan.extent, req);
      return;
    }
    if (plan.kind === "older" && pagingLeft(budget.current)) {
      const part = req.part;
      const cursor = req.cursor;
      const before = store.get().transcripts[a.tk];
      setBusy(true);
      transcripts.loadEarlier(a.sessionKey, a.scope).then(
        () => {
          const grown = grownBy(before, store.get().transcripts[a.tk]);
          budget.current = { events: budget.current.events + grown.events, bytes: budget.current.bytes + grown.bytes };
          setBusy(false);
          // A page that brought nothing is not progress: the window is read detached instead of asking again.
          if (grown.events === 0) setDetached({ part, cursor, events: null });
        },
        () => {
          setBusy(false);
          setDetached({ part, cursor, events: null });
        },
      );
      return;
    }
    setDetached({ part: req.part, cursor: req.cursor, events: null });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `finish` reads only the arguments above and stable state setters.
  }, [req, detached, busy, detIndex, a.live.index, a.live.win.oldest, a.live.win.hasMore, a.live.win.loading, a.earlier.index, a.earlier.win.oldest, a.earlier.win.hasMore, a.earlier.win.loading]);

  // The detached window is read from the host, around the hit.
  useEffect(() => {
    if (!detached || detached.events) return;
    let live = true;
    const { part, cursor } = detached;
    const same = (d: Detached | null) => d !== null && d.part === part && d.cursor === cursor;
    api.history(a.host, { id: a.id, before: cursor + 1, limit: DETACHED_LIMIT, scope: a.scope, earlier: part === "earlier" }).then(
      (page) => {
        if (live) setDetached((d) => (same(d) ? { ...d!, events: page.events } : d));
      },
      (e) => {
        if (live) setDetached((d) => (same(d) ? { ...d!, events: [], error: errorMessage(e) } : d));
      },
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- keyed by the window's identity: its part and hit.
  }, [detached?.part, detached?.cursor, detached?.events === null]);

  // The in-text marks of a hit: applied once its row is in the DOM (the list renders the row after the scroll), then dropped.
  useEffect(() => {
    if (!hl) return;
    const el = rowElement(a.rootRef.current, hl.key);
    if (!el) return;
    setHl(null);
    if (!markText(el, hl.query, hl.hint)) a.listRef.current?.scrollToKey(hl.key, { highlight: true });
  });

  return { jump, backToLatest, unpin, detached, detachedRows, detIndex, pinned, navRef };
}
