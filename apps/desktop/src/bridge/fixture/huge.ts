import { SPECS, type Spec } from "./sessions";

/** A session with thousands of transcript rows, for the virtualised-transcript scroll/visual demo (`?huge=1`). */
export const HUGE_SPEC: Spec = {
  n: 99,
  name: "index-every-repo",
  driver: "claude",
  host: "x1",
  proj: "ledgerline",
  branch: "chore/index-all",
  agoMin: 1,
  cost: 7.4,
  kind: "working",
  line: "Indexing the monorepo, module by module.",
  // ~600 completed turns → several thousand transcript rows (user prompts, thinking, prose, tool rows,
  // and "Done in …" footers). Still working, so live rows stream in at the bottom.
  rounds: 600,
};

/** `?huge=1`: adds the thousands-of-rows working session. */
export function applyHuge() {
  if (!SPECS.some((x) => x.n === HUGE_SPEC.n)) SPECS.push(HUGE_SPEC);
}
