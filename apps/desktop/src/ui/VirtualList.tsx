import { forwardRef, memo, useCallback, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";

export interface VirtualListHandle {
  /** Scrolls the least that brings the row into view (a row is not centred). `highlight` flashes it once. No-op for an unknown key. */
  scrollToKey(key: string, opts?: { highlight?: boolean }): void;
  /** Scrolls to the end. With `stickToEnd` the list keeps following the end from here, until the reader scrolls up. */
  scrollToEnd(): void;
}

export interface VirtualListProps<T> {
  items: T[];
  getKey: (item: T, index: number) => string;
  /** Height guess for rows not measured yet. */
  estimateSize: (item: T, index: number) => number;
  renderItem: (item: T, index: number) => ReactNode;
  className?: string;
  overscan?: number;
  /** Rows rendered when the viewport has no layout (tests, first paint). */
  initialCount?: number;
  /** Which end those rows are taken from: the first ones (default), or the last ones (a list that starts at its bottom). */
  anchor?: "start" | "end";
  /**
   * Keep the bottom in view while the list grows, as long as the reader is within a few pixels of it. Scrolling up releases it.
   * A list with `anchor="end"` also starts at its bottom.
   */
  stickToEnd?: boolean;
  /**
   * Keep the first visible row where it is on screen when rows are added or measured above it: a list that grows at its top
   * (earlier history paged in) does not jump under the reader.
   */
  keepAnchor?: boolean;
  /** Called once each time the reader reaches the start of the list. Not again until the reader has left the start. */
  onReachStart?: () => void;
  role?: string;
  "aria-label"?: string;
}

/** Within this many pixels of the end, the reader counts as at the end (and `stickToEnd` follows it). */
const STICK_PX = 80;
/** Within this many pixels of the top, the reader counts as at the start (`onReachStart`). */
const START_PX = 120;

interface Layout {
  offsets: number[];
  keys: string[];
  index: Map<string, number>;
}

interface RowProps<T> {
  item: T;
  index: number;
  vkey: string;
  /** Flash count of this row (0: not flashed). Odd and even counts differ, so each flash restarts the animation. */
  flash: number;
  renderItem: (item: T, index: number) => ReactNode;
  observe: (el: Element) => void;
  unobserve: (el: Element) => void;
}

/**
 * One mounted row. It is measured while mounted, so a row that grows (a gate, an expanded detail) moves the rows below it.
 *
 * It calls `renderItem` itself and is memoised: a scroll re-renders the list but not a row whose item, index, flash and the
 * stable callbacks did not change, so the row's body (markdown, highlighting) is never recomputed while scrolling. Only rows
 * that actually moved, appeared, or changed render again. `renderItem`, `observe` and `unobserve` must keep stable identities.
 */
function MeasuredRowInner<T>({ item, index, vkey, flash, renderItem, observe, unobserve }: RowProps<T>) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    observe(el);
    return () => unobserve(el);
  }, [observe, unobserve]);
  const flashName = flash === 0 ? undefined : flash % 2 ? "a" : "b";
  return <div ref={ref} data-vkey={vkey} data-flash={flashName}>{renderItem(item, index)}</div>;
}
const MeasuredRow = memo(MeasuredRowInner) as <T>(p: RowProps<T>) => React.ReactElement;

/**
 * Windowing list with variable, measured row heights (rows may grow: inline gates, expanded
 * details). Only rows near the viewport are mounted, so hundreds of sessions stay cheap.
 * The optional behaviours (`stickToEnd`, `keepAnchor`, `onReachStart`) are off by default, so a list that does not ask for
 * them scrolls exactly as it did before they existed.
 */
function VirtualListInner<T>(
  { items, getKey, estimateSize, renderItem, className = "", overscan = 6, initialCount = 40, anchor = "start", stickToEnd = false, keepAnchor = false, onReachStart, role, ...aria }: VirtualListProps<T>,
  ref: React.ForwardedRef<VirtualListHandle>,
) {
  const box = useRef<HTMLDivElement>(null);
  const sizes = useRef(new Map<string, number>());
  const [ver, bump] = useState(0);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewport, setViewport] = useState(0);
  const [flash, setFlash] = useState<{ key: string; n: number } | null>(null);
  const ro = useRef<ResizeObserver | null>(null);

  const { offsets, keys, index } = useMemo(() => {
    const o = new Array<number>(items.length + 1);
    const ks = new Array<string>(items.length);
    const idx = new Map<string, number>();
    o[0] = 0;
    for (let i = 0; i < items.length; i++) {
      const k = getKey(items[i], i);
      ks[i] = k;
      idx.set(k, i);
      o[i + 1] = o[i] + (sizes.current.get(k) ?? estimateSize(items[i], i));
    }
    return { offsets: o, keys: ks, index: idx };
    // sizes is a ref: `bump` triggers recomputation through re-render
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [items, getKey, estimateSize, ver]);
  const total = offsets[items.length];

  let start = anchor === "end" ? Math.max(0, items.length - initialCount) : 0;
  let end = anchor === "end" ? items.length : Math.min(items.length, initialCount);
  if (viewport > 0) {
    let lo = 0;
    let hi = items.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (offsets[mid + 1] <= scrollTop) lo = mid + 1;
      else hi = mid;
    }
    start = Math.max(0, lo - overscan);
    let e = lo;
    while (e < items.length && offsets[e] < scrollTop + viewport) e++;
    end = Math.min(items.length, e + overscan);
  }

  // The observer is made on first use, so rows mounted by the first render are measured too. Its handler only touches refs
  // and the state setters, which do not change, so it is made once.
  const onResize = useCallback((entries: ResizeObserverEntry[]) => {
    const el = box.current;
    let changed = false;
    for (const en of entries) {
      const t = en.target as HTMLElement;
      if (t === el) {
        if (el) setViewport(el.clientHeight);
        continue;
      }
      const k = t.dataset.vkey;
      if (!k) continue;
      const h = Math.round(en.borderBoxSize?.[0]?.blockSize ?? t.offsetHeight);
      if (h > 0 && sizes.current.get(k) !== h) {
        sizes.current.set(k, h);
        changed = true;
      }
    }
    if (changed) bump((n) => n + 1);
  }, []);
  const observer = useCallback((): ResizeObserver | null => {
    if (!ro.current && typeof ResizeObserver !== "undefined") ro.current = new ResizeObserver(onResize);
    return ro.current;
  }, [onResize]);
  const observe = useCallback((el: Element) => observer()?.observe(el), [observer]);
  const unobserve = useCallback((el: Element) => ro.current?.unobserve(el), []);

  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    setViewport(el.clientHeight);
    observe(el);
    return () => ro.current?.unobserve(el);
  }, [observe]);
  useEffect(() => () => {
    ro.current?.disconnect();
    ro.current = null;
  }, []);

  // The last committed layout, and where the reader is in it. Written by the commit effect and by the scroll handler,
  // and read by the handle, so a scroll or a call never sees a layout older than the one on screen.
  const layout = useRef<Layout>({ offsets: [0], keys: [], index: new Map() });
  const follow = useRef({ stuck: stickToEnd, anchor: null as { key: string; screen: number } | null, reached: false });
  const latest = useRef({ onReachStart });

  /** Scrolls to `top`, clamped to the layout's extent, and renders the window for it. */
  const jumpTo = (top: number) => {
    const el = box.current;
    if (!el) return;
    const lay = layout.current;
    const max = Math.max(0, lay.offsets[lay.keys.length] - el.clientHeight);
    el.scrollTop = Math.min(Math.max(0, top), max);
    setScrollTop(el.scrollTop);
  };

  /**
   * Records where the reader is: at the end, which row is first on screen and where, and whether the start is reached.
   * The start is reached once per arrival: `onReachStart` is not called again until the reader is away from it.
   */
  const snapshot = () => {
    const el = box.current;
    if (!el) return;
    const lay = layout.current;
    const top = el.scrollTop;
    const vh = el.clientHeight;
    const total = lay.offsets[lay.keys.length];
    const f = follow.current;
    f.stuck = total - top - vh < STICK_PX;
    let lo = 0;
    let hi = lay.keys.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (lay.offsets[mid + 1] <= top) lo = mid + 1;
      else hi = mid;
    }
    f.anchor = lo < lay.keys.length ? { key: lay.keys[lo], screen: lay.offsets[lo] - top } : null;
    // Without a layout (no viewport yet) there is no start to reach.
    if (vh <= 0) return;
    if (top >= START_PX) f.reached = false;
    else if (!f.reached) {
      f.reached = true;
      latest.current.onReachStart?.();
    }
  };

  // Every commit: first keep the reader where they were (following the end, or holding the first visible row on screen), then
  // record the new position. The layout is the one just committed; the reader's position is the one before it.
  useLayoutEffect(() => {
    layout.current = { offsets, keys, index };
    latest.current = { onReachStart };
    const el = box.current;
    if (!el) return;
    const f = follow.current;
    if (stickToEnd && f.stuck) {
      jumpTo(total);
    } else if (keepAnchor && f.anchor) {
      const i = index.get(f.anchor.key);
      if (i !== undefined) {
        const want = offsets[i] - f.anchor.screen;
        if (Math.abs(want - el.scrollTop) >= 0.5) jumpTo(want);
      }
    }
    snapshot();
  });

  useImperativeHandle(ref, () => ({
    scrollToKey(key: string, opts?: { highlight?: boolean }) {
      const lay = layout.current;
      const el = box.current;
      const i = lay.index.get(key);
      if (i === undefined || !el) return;
      const top = lay.offsets[i];
      const bottom = lay.offsets[i + 1];
      let next = el.scrollTop;
      if (top < next) next = top;
      else if (bottom > next + el.clientHeight) next = bottom - el.clientHeight;
      jumpTo(next);
      snapshot();
      // Each flash is numbered: the row alternates between two animation names, so a repeat restarts the animation.
      if (opts?.highlight) setFlash((f) => ({ key, n: (f?.n ?? 0) + 1 }));
    },
    scrollToEnd() {
      follow.current.stuck = true;
      jumpTo(layout.current.offsets[layout.current.keys.length]);
      snapshot();
    },
  }));

  useEffect(() => {
    // Keep rows that unmounted from leaking measurements forever.
    if (sizes.current.size > items.length * 2 + 50) {
      const live = new Set(keys);
      for (const k of sizes.current.keys()) if (!live.has(k)) sizes.current.delete(k);
    }
  }, [items, keys]);

  return (
    <div
      ref={box}
      className={`vlist ${className}`}
      role={role}
      aria-label={aria["aria-label"]}
      onScroll={() => {
        setScrollTop(box.current?.scrollTop ?? 0);
        snapshot();
      }}
    >
      <div style={{ height: offsets[start] }} aria-hidden="true" />
      {items.slice(start, end).map((it, j) => {
        const i = start + j;
        const k = keys[i];
        return (
          <MeasuredRow<T>
            key={k}
            item={it}
            index={i}
            vkey={k}
            flash={flash?.key === k ? flash.n : 0}
            renderItem={renderItem}
            observe={observe}
            unobserve={unobserve}
          />
        );
      })}
      <div style={{ height: Math.max(0, total - offsets[end]) }} aria-hidden="true" />
    </div>
  );
}

export const VirtualList = forwardRef(VirtualListInner) as <T>(
  p: VirtualListProps<T> & { ref?: React.Ref<VirtualListHandle> },
) => React.ReactElement;
