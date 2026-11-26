import { useEffect, useState } from "react";
import { elapsed } from "../lib/format";

/** Re-render every `ms` while `on` (countdowns). */
export function useNow(on: boolean, ms = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return;
    const h = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(h);
  }, [on, ms]);
  return now;
}

/** How long it has run (running: until `now`), or ran. */
export const taskElapsedMs = (t: { started_at_ms: number; ended_at_ms?: number | null; running: boolean }, now: number): number => Math.max(0, (t.running ? now : t.ended_at_ms ?? now) - t.started_at_ms);

/** How long a task or subagent has run (it ticks while it runs) or ran. */
export function Elapsed({ t }: { t: { started_at_ms: number; ended_at_ms?: number | null; running: boolean } }) {
  const now = useNow(t.running);
  return <>{elapsed(taskElapsedMs(t, now))}</>;
}
