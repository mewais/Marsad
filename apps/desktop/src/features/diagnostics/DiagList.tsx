import { useEffect, useRef, useState } from "react";
import { Icon, VirtualList, type VirtualListHandle } from "../../ui";
import { clockMs, type Tone } from "./words";

/** One row of a diagnostics list: a time, a sentence, and the raw fields behind it. */
export interface DiagRow {
  key: string;
  /** Wall time on the clock of the machine that wrote it. */
  t: number;
  tone: Tone;
  words: string;
  /** Who wrote it, when the list mixes writers (`hub`, `daemon`). */
  source?: string;
  fields: () => { k: string; v: string }[];
}

const estimate = () => 26;
const keyOf = (r: DiagRow) => r.key;

/**
 * A virtualized list of rows in the order given, the newest at the bottom, which stays in view
 * while new rows arrive unless the reader scrolled up. A row opens to show its raw fields.
 */
export function DiagList({ rows, label, height = 320 }: { rows: DiagRow[]; label: string; height?: number }) {
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set());
  const list = useRef<VirtualListHandle>(null);
  const stick = useRef(true);
  const last = rows[rows.length - 1]?.key;
  useEffect(() => {
    if (last !== undefined && stick.current) list.current?.scrollToKey(last);
  }, [last, rows.length]);
  const toggle = (k: string) =>
    setOpen((cur) => {
      const next = new Set(cur);
      if (!next.delete(k)) next.add(k);
      return next;
    });
  return (
    <div
      className="dgl"
      style={{ height }}
      onScrollCapture={(e) => {
        const el = e.target as HTMLElement;
        if (el.classList?.contains("vlist")) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
      }}
    >
      <VirtualList
        ref={list}
        items={rows}
        getKey={keyOf}
        estimateSize={estimate}
        initialCount={60}
        anchor="end"
        role="list"
        aria-label={label}
        renderItem={(r) => (
          <div className={`dgr ${r.tone}`} role="listitem" data-tone={r.tone} data-t={r.t}>
            <button type="button" className="dgr-main" aria-expanded={open.has(r.key)} onClick={() => toggle(r.key)}>
              <span className="dgr-t mono tnum">{clockMs(r.t)}</span>
              <span className="dgr-dot" aria-hidden="true" />
              <span className="dgr-w">{r.words}</span>
              {r.source ? <span className="tag">{r.source}</span> : null}
              <Icon name={open.has(r.key) ? "chev" : "chevr"} size="xs" className="dgr-x" />
            </button>
            {open.has(r.key) ? (
              <dl className="dgr-f mono">
                {r.fields().map(({ k, v }) => (
                  <div key={k} className="contents">
                    <dt>{k}</dt>
                    <dd>{v}</dd>
                  </div>
                ))}
              </dl>
            ) : null}
          </div>
        )}
      />
    </div>
  );
}
