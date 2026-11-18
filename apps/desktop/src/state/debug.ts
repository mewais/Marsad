import { useRef } from "react";

/** Render counters for tests: `useRenderProbe("row:hz/abc")` bumps a count per component render. */
export const renderCounts = new Map<string, number>();

export function useRenderProbe(name: string) {
  renderCounts.set(name, (renderCounts.get(name) ?? 0) + 1);
  return useRef(name).current;
}
