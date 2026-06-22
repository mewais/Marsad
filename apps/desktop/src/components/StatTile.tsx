interface StatTileProps {
  label: string;
  value: string | number;
  sub?: string;
  color?: string;
  border?: string;
  bg?: string;
}

/** One of the 4 stat tiles in the Overview command center. */
export function StatTile({
  label,
  value,
  sub,
  color = "var(--txt)",
  border = "var(--border)",
  bg = "var(--panel)",
}: StatTileProps) {
  return (
    <div
      style={{
        padding: "15px 16px",
        borderRadius: 13,
        border: `1px solid ${border}`,
        background: bg,
      }}
    >
      <div
        style={{
          fontSize: 12,
          fontWeight: 700,
          letterSpacing: 0.4,
          textTransform: "uppercase",
          color,
        }}
      >
        {label}
      </div>
      <div
        style={{
          fontSize: 33,
          fontWeight: 800,
          color,
          lineHeight: 1.1,
          marginTop: 4,
        }}
      >
        {value}
      </div>
      {sub && (
        <div style={{ fontSize: 12, color: "var(--txt2)", marginTop: 2 }}>
          {sub}
        </div>
      )}
    </div>
  );
}
