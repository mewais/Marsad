interface ToggleProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
}

/** iOS-style toggle switch used in Settings and header. */
export function Toggle({ checked, onChange, disabled }: ToggleProps) {
  return (
    <button
      role="switch"
      aria-checked={checked}
      onClick={() => !disabled && onChange(!checked)}
      disabled={disabled}
      style={{
        width: 40,
        height: 23,
        borderRadius: 12,
        border: "none",
        cursor: disabled ? "default" : "pointer",
        background: checked ? "var(--accent)" : "var(--border2)",
        position: "relative",
        flexShrink: 0,
        transition: "background 0.2s",
      }}
    >
      <span
        style={{
          position: "absolute",
          top: 2,
          left: 2,
          width: 19,
          height: 19,
          borderRadius: "50%",
          background: "#fff",
          transform: checked ? "translateX(17px)" : "none",
          boxShadow: "0 1px 3px rgba(0,0,0,0.35)",
          transition: "transform 0.2s",
        }}
      />
    </button>
  );
}
