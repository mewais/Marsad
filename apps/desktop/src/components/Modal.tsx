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
        background: "rgba(0,0,0,0.55)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width,
          maxWidth: "90vw",
          maxHeight: "85vh",
          overflowY: "auto",
          background: "var(--elev)",
          border: "1px solid var(--border2)",
          borderRadius: 16,
          boxShadow: "var(--shadow)",
          animation: "slideup 0.15s ease",
        }}
      >
        {children}
      </div>
    </div>
  );
}
