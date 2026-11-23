import { useCallback, useEffect, useId, useRef, useState } from "react";
import { useEscape } from "./escape";

export interface PopoverTriggerProps {
  "aria-haspopup": "menu" | "listbox" | "dialog";
  "aria-expanded": boolean;
  "aria-controls": string | undefined;
  onClick: () => void;
  ref: React.RefObject<HTMLButtonElement>;
}

export interface PopoverProps {
  trigger: (a: { open: boolean; props: PopoverTriggerProps }) => React.ReactNode;
  children: (a: { close: () => void; id: string }) => React.ReactNode;
  haspopup?: "menu" | "listbox" | "dialog";
  className?: string;
  align?: "left" | "right";
  /** Open upwards (composer pickers). */
  up?: boolean;
  onOpenChange?: (open: boolean) => void;
}

/** Anchored popup: closes on Esc / outside click and returns focus to the trigger. */
export function Popover({ trigger, children, haspopup = "dialog", className = "", align = "right", up, onOpenChange }: PopoverProps) {
  const [open, setOpen] = useState(false);
  const id = useId();
  const wrap = useRef<HTMLDivElement>(null);
  const btn = useRef<HTMLButtonElement>(null);
  const set = useCallback(
    (v: boolean) => {
      setOpen(v);
      onOpenChange?.(v);
    },
    [onOpenChange],
  );
  const close = useCallback(() => {
    set(false);
    btn.current?.focus();
  }, [set]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (wrap.current && !wrap.current.contains(e.target as Node)) set(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open, set]);
  useEscape(open, close);

  return (
    <div ref={wrap} className="relwrap">
      {trigger({
        open,
        props: {
          "aria-haspopup": haspopup,
          "aria-expanded": open,
          "aria-controls": open ? id : undefined,
          onClick: () => set(!open),
          ref: btn,
        },
      })}
      {open ? (
        <div
          id={id}
          className={`popover ${className}`}
          style={{ position: "absolute", zIndex: 45, [up ? "bottom" : "top"]: "calc(100% + 6px)", [align]: 0 }}
        >
          {children({ close, id })}
        </div>
      ) : null}
    </div>
  );
}
