import { SPECS, type Spec } from "./sessions";

/**
 * `?background=1`: a session that works in the background. Its agent started two subagents and a shell command that
 * run on after the turn that started them ended; one subagent has reported back (the agent started a turn of its own
 * for it); two tasks still run, so the session is Working with no turn open. Stop and Interrupt act on them as the
 * daemon's do.
 */
export const BACKGROUND_SPEC: Spec = {
  n: 42, name: "route-auth-audit", driver: "claude", host: "x1", proj: "ledgerline", branch: "chore/auth-audit", agoMin: 1, cost: 1.74, kind: "working",
  line: "Working in the background", backgroundOnly: true,
};

export function applyBackground() {
  if (!SPECS.some((s) => s.n === BACKGROUND_SPEC.n)) SPECS.push(BACKGROUND_SPEC);
}
