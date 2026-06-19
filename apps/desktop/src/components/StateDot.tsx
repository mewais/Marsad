import type { Phase } from "../bridge/types";
import { phaseColor } from "./Badge";

interface StateDotProps {
  state: Phase;
  size?: number;
}

/** Colored dot that pulses for Working/Needs states. */
export function StateDot({ state, size = 8 }: StateDotProps) {
  const pulsing = state === "Working" || state === "Starting";
  const needsPulse = state === "Needs";

  return (
    <span
      style={{
        width: size,
        height: size,
        borderRadius: "50%",
        background: phaseColor[state],
        display: "inline-block",
        flexShrink: 0,
        animation: pulsing
          ? "dotpulse 1.3s infinite"
          : needsPulse
          ? "dotpulse 1.4s infinite"
          : undefined,
      }}
    />
  );
}
