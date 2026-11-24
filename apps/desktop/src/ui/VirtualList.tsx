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
/** A scroll is treated as the reader's own for this long after a wheel/key/pointer/touch gesture (covers inertial scrolling). */
const GESTURE_MS = 250;

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
    if (followRaf.current) cancelAnimationFrame(followRaf.current);
  }, []);

  // The last committed layout, and where the reader is in it. Written by the commit effect and by the scroll handler,
  // and read by the handle, so a scroll or a call never sees a layout older than the one on screen.
  const layout = useRef<Layout>({ offsets: [0], keys: [], index: new Map() });
  // `following` is an explicit mode, not a geometry read: it is released only by a genuine reader gesture that leaves the end
  // (see the scroll handler), and re-engaged when the reader returns within STICK_PX of the end. A programmatic jump or a
  // measurement reflow that clamps the scroll never changes it, so a settling list of thousands of rows cannot shake it loose.
  const follow = useRef({ following: stickToEnd, anchor: null as { key: string; screen: number } | null, reached: false });
  const latest = useRef({ onReachStart });
  // When the reader last drove the scroll by a wheel/key/pointer/touch gesture. Scroll events close to it are the reader's;
  // the rest (our jumps, measurement reflows) are not, and must not release following.
  const lastGesture = useRef(-Infinity);

  const followRaf = useRef(0);
  const followBurst = useRef(0);

  /**
   * The scroll position of the true end: the real rendered extent (`scrollHeight`) when the browser gives it, else the list's
   * layout total (tests, first paint). The "already at the end" guard and the follow jump compare against it with a 1px tolerance.
   */
  const endTop = (el: HTMLElement): number => {
    const base = el.scrollHeight > 0 ? el.scrollHeight : layout.current.offsets[layout.current.keys.length];
    return Math.max(0, base - el.clientHeight);
  };

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
   * Bring the end to the bottom of the viewport when it has dropped below it (content grew), as a DOM side effect, then sync the
   * window. Converges synchronously for the common case; but if re-pinning does not settle within a few synchronous passes — a
   * real scrollbar reflow can make the rendered extent wobble as the boundary row mounts and unmounts — hand the rest off to an
   * animation frame, so the correction becomes one-per-frame and cannot recurse into a React #185 update loop. When already at
   * the end (1px tolerance) nothing changes and no re-render is scheduled, so a settled list does no more work.
   */
  const followEnd = (el: HTMLElement) => {
    const max = endTop(el);
    if (max - el.scrollTop > 1) el.scrollTop = max;
    if (el.scrollTop === scrollTop) { followBurst.current = 0; return; }
    if (followBurst.current < 3) {
      followBurst.current += 1;
      setScrollTop(el.scrollTop);
    } else {
      requestFollowEnd();
    }
  };

  /** Follow the end on the next animation frame (coalesced), after resetting the synchronous burst count. */
  const requestFollowEnd = () => {
    if (followRaf.current || typeof requestAnimationFrame === "undefined") return;
    followRaf.current = requestAnimationFrame(() => {
      followRaf.current = 0;
      followBurst.current = 0;
      const el = box.current;
      if (el && stickToEnd && follow.current.following) followEnd(el);
    });
  };

  /**
   * Records which row is first on screen and where, and whether the start is reached. (Following the end is tracked separately,
   * by the scroll handler, so geometry never releases it.) The start is reached once per arrival: `onReachStart` is not called
   * again until the reader is away from it.
   */
  const snapshot = () => {
    const el = box.current;
    if (!el) return;
    const lay = layout.current;
    const top = el.scrollTop;
    const vh = el.clientHeight;
    const f = follow.current;
    let lo = 0;
    let hi = lay.keys.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (lay.offsets[mid + 1] <= top) lo = mid + 1;
      else hi = mid;
    }
    f.anchor = lo < lay.keys.length ? { key: lay.keys[lo], screen: lay.offsets[lo] - top } : null;
    // Without a layout (no viewport yet) there is no start to reach. A list that is following its end is not at its start, even
    // on the first frame before the follow jump has run, so it never reaches the start while following.
    if (vh <= 0) return;
    if (top >= START_PX) f.reached = false;
    else if (!f.reached && !(stickToEnd && f.following)) {
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
    if (stickToEnd && f.following) {
      followEnd(el);
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
      follow.current.following = true;
      const el = box.current;
      if (el) followEnd(el);
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
      // A gesture marks the scrolls that follow as the reader's own (covers inertial scrolling), so only the reader — never a
      // programmatic jump or a measurement reflow — can release following.
      onWheel={() => (lastGesture.current = performance.now())}
      onKeyDown={() => (lastGesture.current = performance.now())}
      onPointerDown={() => (lastGesture.current = performance.now())}
      onTouchStart={() => (lastGesture.current = performance.now())}
      onTouchMove={() => (lastGesture.current = performance.now())}
      onScroll={() => {
        const el = box.current;
        if (!el) return;
        const top = el.scrollTop;
        setScrollTop(top);
        if (stickToEnd) {
          const f = follow.current;
          const nearEnd = endTop(el) - top <= STICK_PX;
          // Release only on a genuine reader gesture that left the end; re-engage when the reader returns to the end. Our own
          // jumps and measurement reflows (no recent gesture) never release it.
          if (f.following) {
            if (!nearEnd && performance.now() - lastGesture.current < GESTURE_MS) f.following = false;
          } else if (nearEnd) {
            f.following = true;
          }
        }
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
