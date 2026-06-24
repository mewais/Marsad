import type { Phase } from "../bridge/types";

interface BadgeProps {
  state: Phase;
  label?: string;
}

const phaseColor: Record<Phase, string> = {
  Needs: "var(--needs)",
  Working: "var(--accent)",
  Error: "var(--error)",
  Done: "var(--done)",
  Starting: "var(--accent2)",
  Stale: "var(--neutral)",
  Detached: "var(--txt3)",
};

const phaseBg: Record<Phase, string> = {
  Needs: "var(--needs-bg)",
  Working: "var(--working-bg)",
  Error: "var(--error-bg)",
  Done: "var(--done-bg)",
  Starting: "var(--working-bg)",
  Stale: "transparent",
  Detached: "transparent",
};

const phaseLabel: Record<Phase, string> = {
  Needs: "Needs you",
  Working: "Working",
  Error: "Error",
  Done: "Done",
  Starting: "Starting",
  Stale: "Stale",
  Detached: "Detached",
};

export function Badge({ state, label }: BadgeProps) {
  return (
    <span
      style={{
        fontSize: "calc(12.5px * var(--fs,1))",
        fontWeight: 700,
        letterSpacing: 0.4,
        padding: "3px 8px",
        borderRadius: 6,
        color: phaseColor[state],
        background: phaseBg[state],
        textTransform: "uppercase",
        whiteSpace: "nowrap",
        flexShrink: 0,
      }}
    >
      {label ?? phaseLabel[state]}
    </span>
  );
}

export { phaseColor, phaseBg, phaseLabel };
