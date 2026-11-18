import { useId, useRef } from "react";

export interface TabDef {
  id: string;
  label: string;
  count?: number | string;
}

export interface TabsProps {
  tabs: TabDef[];
  value: string;
  onChange: (id: string) => void;
  label: string;
  /** Rendered as the tab panel for the active tab. */
  children?: React.ReactNode;
  className?: string;
}

/** WAI-ARIA tabs: arrow keys move focus + selection, Home/End jump. */
export function Tabs({ tabs, value, onChange, label, children, className = "" }: TabsProps) {
  const base = useId();
  const list = useRef<HTMLDivElement>(null);
  const onKeyDown = (e: React.KeyboardEvent) => {
    const i = tabs.findIndex((t) => t.id === value);
    let n = -1;
    if (e.key === "ArrowRight") n = (i + 1) % tabs.length;
    else if (e.key === "ArrowLeft") n = (i - 1 + tabs.length) % tabs.length;
    else if (e.key === "Home") n = 0;
    else if (e.key === "End") n = tabs.length - 1;
    if (n < 0) return;
    e.preventDefault();
    onChange(tabs[n].id);
    list.current?.querySelectorAll<HTMLElement>('[role="tab"]')[n]?.focus();
  };
  return (
    <>
      <div ref={list} className={`tabs-list ${className}`} role="tablist" aria-label={label} onKeyDown={onKeyDown}>
        {tabs.map((t) => (
          <button
            key={t.id}
            id={`${base}-t-${t.id}`}
            role="tab"
            type="button"
            aria-selected={t.id === value}
            aria-controls={`${base}-p`}
            tabIndex={t.id === value ? 0 : -1}
            onClick={() => onChange(t.id)}
          >
            {t.label}
            {t.count != null ? <span className="n">{t.count}</span> : null}
          </button>
        ))}
      </div>
      {children !== undefined ? (
        <div id={`${base}-p`} role="tabpanel" aria-labelledby={`${base}-t-${value}`} className="contents">
          {children}
        </div>
      ) : null}
    </>
  );
}
