import { forwardRef, useCallback, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState } from "react";

export interface VirtualListHandle {
  scrollToKey(key: string): void;
}

export interface VirtualListProps<T> {
  items: T[];
  getKey: (item: T, index: number) => string;
  /** Height guess for rows not measured yet. */
  estimateSize: (item: T, index: number) => number;
  renderItem: (item: T, index: number) => React.ReactNode;
  className?: string;
  overscan?: number;
  /** Rows rendered when the viewport has no layout (tests, first paint). */
  initialCount?: number;
  /** Which end those rows are taken from: the first ones (default), or the last ones (a list that starts at its bottom). */
  anchor?: "start" | "end";
  role?: string;
  "aria-label"?: string;
}

/**
 * Windowing list with variable, measured row heights (rows may grow: inline gates, expanded
 * details). Only rows near the viewport are mounted, so hundreds of sessions stay cheap.
 */
function VirtualListInner<T>(
  { items, getKey, estimateSize, renderItem, className = "", overscan = 6, initialCount = 40, anchor = "start", role, ...aria }: VirtualListProps<T>,
  ref: React.ForwardedRef<VirtualListHandle>,
) {
  const box = useRef<HTMLDivElement>(null);
  const sizes = useRef(new Map<string, number>());
  const [ver, bump] = useState(0);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewport, setViewport] = useState(0);
  const ro = useRef<ResizeObserver | null>(null);

  const offsets = useMemo(() => {
    const o = new Array<number>(items.length + 1);
    o[0] = 0;
    for (let i = 0; i < items.length; i++) {
      o[i + 1] = o[i] + (sizes.current.get(getKey(items[i], i)) ?? estimateSize(items[i], i));
    }
    return o;
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

  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    setViewport(el.clientHeight);
    if (typeof ResizeObserver === "undefined") return;
    const o = new ResizeObserver((entries) => {
      let changed = false;
      for (const en of entries) {
        const t = en.target as HTMLElement;
        if (t === el) {
          setViewport(el.clientHeight);
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
    });
    ro.current = o;
    o.observe(el);
    return () => o.disconnect();
  }, []);

  const measure = useCallback((el: HTMLDivElement | null) => {
    if (el && ro.current) ro.current.observe(el);
  }, []);

  useImperativeHandle(ref, () => ({
    scrollToKey(key: string) {
      const i = items.findIndex((it, idx) => getKey(it, idx) === key);
      const el = box.current;
      if (i < 0 || !el) return;
      const top = offsets[i];
      const bottom = offsets[i + 1];
      if (top < el.scrollTop) el.scrollTop = top;
      else if (bottom > el.scrollTop + el.clientHeight) el.scrollTop = bottom - el.clientHeight;
    },
  }));

  useEffect(() => {
    // Keep rows that unmounted from leaking measurements forever.
    if (sizes.current.size > items.length * 2 + 50) {
      const live = new Set(items.map(getKey));
      for (const k of sizes.current.keys()) if (!live.has(k)) sizes.current.delete(k);
    }
  }, [items, getKey]);

  return (
    <div
      ref={box}
      className={`vlist ${className}`}
      role={role}
      aria-label={aria["aria-label"]}
      onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
    >
      <div style={{ height: offsets[start] }} aria-hidden="true" />
      {items.slice(start, end).map((it, j) => {
        const i = start + j;
        const k = getKey(it, i);
        return (
          <div key={k} data-vkey={k} ref={measure}>
            {renderItem(it, i)}
          </div>
        );
      })}
      <div style={{ height: Math.max(0, total - offsets[end]) }} aria-hidden="true" />
    </div>
  );
}

export const VirtualList = forwardRef(VirtualListInner) as <T>(
  p: VirtualListProps<T> & { ref?: React.Ref<VirtualListHandle> },
) => React.ReactElement;
