interface ChipProps {
  label: string;
  count?: number;
  active?: boolean;
  onClick?: () => void;
}

export function Chip({ label, count, active = false, onClick }: ChipProps) {
  return (
    <button
      onClick={onClick}
      style={{
        fontFamily: "inherit",
        fontSize: 12.5,
        fontWeight: 600,
        padding: "5px 10px",
        borderRadius: 7,
        cursor: "pointer",
        border: active ? "1px solid var(--accent)" : "1px solid var(--border)",
        background: active ? "var(--accent-bg)" : "transparent",
        color: active ? "var(--accent2)" : "var(--txt2)",
      }}
    >
      {label}
      {count !== undefined && (
        <span style={{ marginLeft: 5, opacity: 0.75 }}>{count}</span>
      )}
    </button>
  );
}
