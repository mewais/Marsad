import type { Phase, EndReason } from "../bridge/types";

interface BadgeProps {
  state: Phase;
  /** The session's end reason, so a user cancel reads "Cancelled" (calm) instead
   *  of a plain "Done". Ignored unless `state` is Done. */
  endReason?: EndReason | null;
  /** Liveness (two-axis). LIVENESS-FIRST: when `false` (Paused, not running) the
   *  badge reads "Paused" and overrides the activity phase entirely — a paused
   *  session's gate/error is wasted, so it never shows Needs/Error/Done/Working.
   *  Missing/true → show the activity phase as usual. */
  live?: boolean;
  label?: string;
}

const phaseColor: Record<Phase, string> = {
  Needs: "var(--needs)",
  Working: "var(--accent)",
  Error: "var(--error)",
  Done: "var(--done)",
  // Starting is folded into Done (idle) — same colour, not its own thing.
  Starting: "var(--done)",
  Stale: "var(--neutral)",
  Detached: "var(--txt3)",
};

const phaseBg: Record<Phase, string> = {
  Needs: "var(--needs-bg)",
  Working: "var(--working-bg)",
  Error: "var(--error-bg)",
  Done: "var(--done-bg)",
  Starting: "var(--done-bg)",
  Stale: "transparent",
  Detached: "transparent",
};

const phaseLabel: Record<Phase, string> = {
  Needs: "Needs you",
  Working: "Working",
  Error: "Error",
  Done: "Done",
  // Starting folded into Done: a launched-but-idle session reads "Done" (idle).
  Starting: "Done",
  // `Stale` is the internal phase for a not-live session (user-stopped, restored
  // on relaunch, or process gone). Surface it to users as the plainer "Stopped".
  Stale: "Stopped",
  Detached: "Stopped",
};

export function Badge({ state, endReason, live, label }: BadgeProps) {
  // Liveness-first: a not-running session reads "Paused" regardless of its last
  // activity phase.
  const paused = live === false;
  // A user cancel is a calm end, not a failure: it lands in Done but reads
  // "Cancelled" (neutral colour) so it's distinguishable from a genuine finish.
  const isCancelled =
    !paused && (state === "Done" || state === "Starting") && endReason === "Cancelled";
  const color = paused ? "var(--txt3)" : isCancelled ? "var(--txt2)" : phaseColor[state];
  const background = paused ? "var(--panel2)" : isCancelled ? "var(--panel2)" : phaseBg[state];
  const text =
    label ?? (paused ? "Paused" : isCancelled ? "Cancelled" : phaseLabel[state]);
  return (
    <span
      style={{
        fontSize: "calc(12.5px * var(--fs,1))",
        fontWeight: 700,
        letterSpacing: 0.4,
        padding: "3px 8px",
        borderRadius: 6,
        color,
        background,
        textTransform: "uppercase",
        whiteSpace: "nowrap",
        flexShrink: 0,
      }}
    >
      {text}
    </span>
  );
}

export { phaseColor, phaseBg, phaseLabel };
