import { useEffect, useId, useRef } from "react";
import { createPortal } from "react-dom";
import { Button, IconButton } from "./Button";
import { useEscape } from "./escape";

const FOCUSABLE =
  'a[href],button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex="-1"])';

export interface DialogProps {
  open: boolean;
  onClose: () => void;
  title: string;
  children: React.ReactNode;
  footer?: React.ReactNode;
  /** Hide the visual title (still the accessible name), e.g. the command palette. */
  bare?: boolean;
  className?: string;
  /** Element to focus on open; defaults to the first focusable inside. */
  initialFocus?: React.RefObject<HTMLElement>;
  /** Shown next to the title (muted). */
  subtitle?: React.ReactNode;
  /** Extra header content before the close button (e.g. step indicator). */
  headerExtra?: React.ReactNode;
  /** Dialog width in px (max; shrinks with the window). */
  width?: number;
  /** Esc and a click on the scrim do not close it, and there is no close button: it ends only through its own actions. */
  persistent?: boolean;
}

/**
 * Modal dialog: role=dialog + aria-modal, focus moves in and is trapped, Esc and scrim click close
 * (unless `persistent`; Esc goes through the shared escape stack, so anything opened on top of the
 * dialog gets the first Esc), focus returns to the opener. The body scrolls inside the dialog (max-height 100%), so short
 * windows never push the footer off-screen.
 */
export function Dialog({ open, onClose, title, children, footer, bare, className = "", initialFocus, subtitle, headerExtra, width, persistent }: DialogProps) {
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  // A persistent dialog still owns Esc (and swallows it) so nothing underneath reacts to it.
  useEscape(open, () => {
    if (!persistent) closeRef.current();
  });

  useEffect(() => {
    if (!open) return;
    const opener = document.activeElement as HTMLElement | null;
    const el = ref.current!;
    (initialFocus?.current ?? el.querySelector<HTMLElement>(FOCUSABLE) ?? el).focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Tab") {
        const f = Array.from(el.querySelectorAll<HTMLElement>(FOCUSABLE));
        if (f.length === 0) {
          e.preventDefault();
          return;
        }
        const first = f[0];
        const last = f[f.length - 1];
        const active = document.activeElement;
        if (e.shiftKey && (active === first || !el.contains(active))) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && (active === last || !el.contains(active))) {
          e.preventDefault();
          first.focus();
        }
      }
    };
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("keydown", onKey, true);
      opener?.focus?.();
    };
  }, [open, initialFocus]);

  if (!open) return null;
  return createPortal(
    <div
      className="scrim"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget && !persistent) onClose();
      }}
    >
      <div ref={ref} className={`dlg ${className}`} role="dialog" aria-modal="true" aria-labelledby={titleId} tabIndex={-1} style={width ? { width: `min(${width}px, 100%)` } : undefined}>
        {bare ? (
          <h2 id={titleId} className="sr-only">
            {title}
          </h2>
        ) : (
          <div className="dlg-h">
            <h2 id={titleId}>{title}</h2>
            {subtitle ? <span className="faint dlg-sub">{subtitle}</span> : null}
            {headerExtra}
            <span className="sp" />
            {persistent ? null : <IconButton icon="x" label="Close" onClick={onClose} />}
          </div>
        )}
        <div className="dlg-b">{children}</div>
        {footer ? <div className="dlg-f">{footer}</div> : null}
      </div>
    </div>,
    document.body,
  );
}

export interface ConfirmDialogProps {
  open: boolean;
  title: string;
  children: React.ReactNode;
  confirmLabel: string;
  /** Destructive actions get the "deny" (red) style. */
  danger?: boolean;
  pending?: boolean;
  onConfirm: () => void;
  onClose: () => void;
}

/** In-app confirmation (never `window.confirm`): focus starts on Cancel. */
export function ConfirmDialog({ open, title, children, confirmLabel, danger, pending, onConfirm, onClose }: ConfirmDialogProps) {
  const cancel = useRef<HTMLButtonElement>(null);
  return (
    <Dialog open={open} onClose={onClose} title={title} width={460} initialFocus={cancel}
      footer={<><span className="sp" /><Button ref={cancel} onClick={onClose}>Cancel</Button><Button variant={danger ? "deny" : "primary"} pending={pending} onClick={onConfirm}>{confirmLabel}</Button></>}>
      <div className="confirm-body">{children}</div>
    </Dialog>
  );
}
