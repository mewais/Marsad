import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Icon } from "./Icon";
import { useEscape } from "./escape";

export interface SelectOption {
  value: string;
  label: string;
  description?: string;
  /** The label is an identifier (a model id): set in the mono face. */
  mono?: boolean;
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

/** Room between `el` and the edges of everything that clips it (scroll containers, the window). */
function roomAround(el: HTMLElement): { above: number; below: number } {
  const r = el.getBoundingClientRect();
  let top = 0;
  let bottom = window.innerHeight;
  for (let p = el.parentElement; p; p = p.parentElement) {
    const o = getComputedStyle(p).overflowY;
    if (o !== "visible") {
      const b = p.getBoundingClientRect();
      top = Math.max(top, b.top);
      bottom = Math.min(bottom, b.bottom);
    }
  }
  return { above: r.top - top, below: bottom - r.bottom };
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
  const list = useRef<HTMLDivElement>(null);
  // Opens on the side that has room: a menu near the bottom of a scrolling dialog must not be cut off.
  const [flip, setFlip] = useState(false);
  const canSearch = searchable ?? options.length > 7;
  const shown = useMemo(() => {
    const n = q.trim().toLowerCase();
    return n ? options.filter((o) => (o.label + " " + (o.description ?? "") + " " + o.value).toLowerCase().includes(n)) : options;
  }, [options, q]);
  const current = options.find((o) => o.value === value);
  useLayoutEffect(() => {
    if (!open || !wrap.current || !list.current) return setFlip(false);
    const { above, below } = roomAround(wrap.current);
    const need = list.current.offsetHeight + 4;
    const room = up ? above : below;
    setFlip(room < need && (up ? below : above) > room);
  }, [open, up]);
  useEscape(open, () => {
    setOpen(false);
    btn.current?.focus();
  });

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
        <div ref={list} className={`combo-list${!!up !== flip ? " up" : ""}`} onKeyDown={onKey}>
          {canSearch ? (
            <input
              ref={input}
              role="combobox"
              aria-label={`Filter ${label}`}
              aria-expanded="true"
              aria-controls={id}
              aria-activedescendant={shown[active] ? `${id}-${active}` : undefined}
              className="input combo-filter"
             
              value={q}
              placeholder="Filter…"
              onChange={(e) => {
                setQ(e.target.value);
                setActive(0);
              }}
            />
          ) : null}
          <div id={id} role="listbox" aria-label={label} tabIndex={canSearch ? -1 : 0} ref={(el) => (!canSearch && el ? el.focus() : undefined)}>
            {shown.length === 0 ? <div className="faint pad8">No matches</div> : null}
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
                {o.value === value ? <Icon name="check" size="xs" /> : <span className="w12" />}
                <span className="combo-t">
                  <span className={`combo-l${o.mono ? " mono" : ""}`} title={o.label}>{o.label}</span>
                  {o.description ? <span className="d">{o.description}</span> : null}
                </span>
              </div>
            ))}
          </div>
        </div>
      ) : null}
    </div>
  );
}
