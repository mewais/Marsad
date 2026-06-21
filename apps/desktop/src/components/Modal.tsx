import React, { useEffect } from "react";

interface ModalProps {
  open: boolean;
  onClose: () => void;
  children: React.ReactNode;
  width?: number | string;
}

/** Backdrop + centered sheet modal. */
export function Modal({ open, onClose, children, width = 560 }: ModalProps) {
  // Close on Escape
  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [open, onClose]);

  if (!open) return null;

  return (
    <div
      onClick={onClose}
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 100,
        // Design backdrop: deep + slightly blurred (matches GateOverlay).
        background: "rgba(5,7,10,0.62)",
        backdropFilter: "blur(3px)",
        WebkitBackdropFilter: "blur(3px)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        padding: 32,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width,
          maxWidth: "90vw",
          maxHeight: "88vh",
          overflowY: "auto",
          // Design modal surface is --panel (not --elev), 18px radius.
          background: "var(--panel)",
          border: "1px solid var(--border2)",
          borderRadius: 18,
          boxShadow: "var(--shadow)",
          animation: "slideup 0.15s ease",
        }}
      >
        {children}
      </div>
    </div>
  );
}
