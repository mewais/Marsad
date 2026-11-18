import { useCallback, useEffect, useId, useRef, useState } from "react";

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
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        close();
      }
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey, true);
    };
  }, [open, close, set]);

  return (
    <div ref={wrap} style={{ position: "relative", display: "inline-flex" }}>
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
