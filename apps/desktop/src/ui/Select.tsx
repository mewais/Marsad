import { useEffect, useId, useMemo, useRef, useState } from "react";
import { Icon } from "./Icon";

export interface SelectOption {
  value: string;
  label: string;
  description?: string;
}

export interface SelectProps {
  label: string;
  value: string | undefined;
  options: SelectOption[];
  onChange: (value: string) => void;
  placeholder?: string;
  /** Show a filter box (combobox pattern). Default: on when there are more than 7 options. */
  searchable?: boolean;
  className?: string;
  /** Open upwards (composer pickers). */
  up?: boolean;
  /** Render the closed control compactly as a composer "pick" chip. */
  compact?: boolean;
  leading?: React.ReactNode;
  /** Class of the closed button (e.g. "chip" for toolbar filters). */
  buttonClass?: string;
  /** Text shown in the closed button instead of the selected label. */
  display?: string;
}

/** Select / combobox: button opens a listbox; typing filters; Up/Down/Enter/Esc. */
export function Select({ label, value, options, onChange, placeholder = "Select…", searchable, className = "", up, compact, leading, buttonClass, display }: SelectProps) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const [active, setActive] = useState(0);
  const id = useId();
  const wrap = useRef<HTMLDivElement>(null);
  const btn = useRef<HTMLButtonElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const canSearch = searchable ?? options.length > 7;
  const shown = useMemo(() => {
    const n = q.trim().toLowerCase();
    return n ? options.filter((o) => (o.label + " " + (o.description ?? "") + " " + o.value).toLowerCase().includes(n)) : options;
  }, [options, q]);
  const current = options.find((o) => o.value === value);

  useEffect(() => {
    if (!open) return;
    setQ("");
    setActive(Math.max(0, options.findIndex((o) => o.value === value)));
    const t = setTimeout(() => input.current?.focus(), 0);
    const onDown = (e: MouseEvent) => {
      if (wrap.current && !wrap.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => {
      clearTimeout(t);
      document.removeEventListener("mousedown", onDown);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const choose = (o: SelectOption | undefined) => {
    if (!o) return;
    onChange(o.value);
    setOpen(false);
    btn.current?.focus();
  };
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(shown.length - 1, a + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(0, a - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(shown[active]);
    } else if (e.key === "Escape") {
      e.stopPropagation();
      setOpen(false);
      btn.current?.focus();
    }
  };

  return (
    <div ref={wrap} className={`combo ${className}`}>
      <button
        ref={btn}
        type="button"
        className={buttonClass ?? (compact ? "pick" : "selbox")}
        aria-label={`${label}: ${current?.label ?? placeholder}`}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        onClick={() => setOpen((o) => !o)}
        onKeyDown={(e) => {
          if (!open && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
            e.preventDefault();
            setOpen(true);
          }
        }}
      >
        {leading}
        <span>{display ?? current?.label ?? placeholder}</span>
        <Icon name="chev" size={compact ? "xs" : "sm"} />
      </button>
      {open ? (
        <div className="combo-list" style={up ? { top: "auto", bottom: "calc(100% + 4px)", minWidth: 220 } : { minWidth: 220 }} onKeyDown={onKey}>
          {canSearch ? (
            <input
              ref={input}
              role="combobox"
              aria-label={`Filter ${label}`}
              aria-expanded="true"
              aria-controls={id}
              aria-activedescendant={shown[active] ? `${id}-${active}` : undefined}
              className="input"
              style={{ width: "100%", marginBottom: 4, padding: "0 10px", background: "var(--sunken)", border: "1px solid var(--line-2)", borderRadius: 6 }}
              value={q}
              placeholder="Filter…"
              onChange={(e) => {
                setQ(e.target.value);
                setActive(0);
              }}
            />
          ) : null}
          <div id={id} role="listbox" aria-label={label} tabIndex={canSearch ? -1 : 0} ref={(el) => (!canSearch && el ? el.focus() : undefined)}>
            {shown.length === 0 ? <div className="faint" style={{ padding: 8 }}>No matches</div> : null}
            {shown.map((o, i) => (
              <div
                key={o.value}
                id={`${id}-${i}`}
                role="option"
                aria-selected={i === active}
                className="combo-opt"
                onMouseEnter={() => setActive(i)}
                onClick={() => choose(o)}
              >
                {o.value === value ? <Icon name="check" size="xs" /> : <span style={{ width: 12 }} />}
                <span>{o.label}</span>
                {o.description ? <span className="d">{o.description}</span> : null}
              </div>
            ))}
          </div>
        </div>
      ) : null}
    </div>
  );
}
