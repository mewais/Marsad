import type { SlashEntry } from "../bridge/types";

interface SlashPopoverProps {
  entries: SlashEntry[];
  onSelect: (entry: SlashEntry) => void;
  empty?: boolean;
}

/** Slash command popover shown above the composer. */
export function SlashPopover({ entries, onSelect, empty }: SlashPopoverProps) {
  return (
    <div
      style={{
        background: "var(--elev)",
        border: "1px solid var(--border2)",
        borderRadius: 12,
        boxShadow: "var(--shadow)",
        overflow: "hidden",
        maxHeight: 284,
        overflowY: "auto",
      }}
    >
      <div
        style={{
          padding: "8px 13px",
          fontSize: "calc(12.5px * var(--fs,1))",
          fontWeight: 700,
          letterSpacing: 0.5,
          textTransform: "uppercase",
          color: "var(--txt3)",
          borderBottom: "1px solid var(--border)",
        }}
      >
        Slash commands
      </div>
      {entries.map((entry, i) => (
        <div
          key={entry.name}
          onClick={() => onSelect(entry)}
          style={{
            display: "flex",
            alignItems: "center",
            gap: 12,
            padding: "9px 13px",
            cursor: "pointer",
            background: i === 0 ? "var(--accent-bg)" : "transparent",
          }}
          onMouseEnter={(e) => {
            (e.currentTarget as HTMLElement).style.background = "var(--accent-bg)";
          }}
          onMouseLeave={(e) => {
            (e.currentTarget as HTMLElement).style.background =
              i === 0 ? "var(--accent-bg)" : "transparent";
          }}
        >
          <span
            className="mono"
            style={{
              fontSize: "calc(14.5px * var(--fs,1))",
              fontWeight: 600,
              color: "var(--accent2)",
              minWidth: 92,
              flexShrink: 0,
            }}
          >
            {entry.name}
          </span>
          <span style={{ fontSize: "calc(14px * var(--fs,1))", color: "var(--txt2)" }}>
            {entry.desc}
          </span>
        </div>
      ))}
      {empty && (
        <div style={{ padding: "11px 13px", fontSize: "calc(14px * var(--fs,1))", color: "var(--txt3)" }}>
          No matching command
        </div>
      )}
    </div>
  );
}
