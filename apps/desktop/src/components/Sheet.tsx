import React from "react";

interface SheetProps {
  children: React.ReactNode;
  style?: React.CSSProperties;
}

/** Generic card/sheet container matching the .dc.html panel styling. */
export function Sheet({ children, style }: SheetProps) {
  return (
    <div
      style={{
        background: "var(--panel)",
        border: "1px solid var(--border)",
        borderRadius: 13,
        ...style,
      }}
    >
      {children}
    </div>
  );
}
