/**
 * Dropdown — a styled select that matches the workspace switcher menu
 * (trigger pill + elevated popover list). Use instead of a raw <select> so
 * dropdowns look consistent across the app.
 */
import { useState } from "react";

export interface DropdownOption {
  value: string;
  label: string;
}

interface DropdownProps {
  value: string;
  options: DropdownOption[];
  onChange: (value: string) => void;
  disabled?: boolean;
  width?: number | string;
  /** Which edge the popover aligns to. Default "left". */
  align?: "left" | "right";
  testId?: string;
}

export function Dropdown({
  value,
  options,
  onChange,
  disabled = false,
  width = 240,
  align = "left",
  testId,
}: DropdownProps) {
  const [open, setOpen] = useState(false);
  const selected = options.find((o) => o.value === value);

  return (
    <div style={{ position: "relative", flexShrink: 0 }}>
      <button
        type="button"
        data-testid={testId}
        disabled={disabled}
        onClick={() => !disabled && setOpen((o) => !o)}
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          width,
          background: "var(--panel2)",
          border: "1px solid var(--border)",
          borderRadius: 9,
          padding: "8px 11px",
          cursor: disabled ? "default" : "pointer",
          color: "var(--txt)",
          fontFamily: "inherit",
          fontSize: 12.5,
          opacity: disabled ? 0.45 : 1,
          textAlign: "left",
        }}
      >
        <span
          style={{
            flex: 1,
            fontWeight: 600,
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {selected?.label ?? ""}
        </span>
        <span style={{ fontSize: 9, color: "var(--txt3)" }}>▾</span>
      </button>

      {open && !disabled && (
        <>
          {/* click-away backdrop */}
          <div
            onClick={() => setOpen(false)}
            style={{ position: "fixed", inset: 0, zIndex: 54 }}
          />
          <div
            style={{
              position: "absolute",
              top: "calc(100% + 6px)",
              left: align === "left" ? 0 : undefined,
              right: align === "right" ? 0 : undefined,
              minWidth: width,
              maxHeight: 280,
              overflowY: "auto",
              background: "var(--elev)",
              border: "1px solid var(--border2)",
              borderRadius: 12,
              boxShadow: "var(--shadow)",
              zIndex: 55,
              padding: 5,
              animation: "slideup .15s ease",
            }}
          >
            {options.map((o) => {
              const sel = o.value === value;
              return (
                <div
                  key={o.value}
                  onClick={() => {
                    onChange(o.value);
                    setOpen(false);
                  }}
                  style={{
                    padding: "8px 10px",
                    borderRadius: 8,
                    cursor: "pointer",
                    fontSize: 12.5,
                    color: sel ? "var(--accent2)" : "var(--txt)",
                    background: sel ? "var(--accent-bg)" : "transparent",
                    whiteSpace: "nowrap",
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                  }}
                  onMouseEnter={(e) => {
                    if (!sel) e.currentTarget.style.background = "var(--panel2)";
                  }}
                  onMouseLeave={(e) => {
                    if (!sel) e.currentTarget.style.background = "transparent";
                  }}
                >
                  {o.label}
                </div>
              );
            })}
          </div>
        </>
      )}
    </div>
  );
}
