import { render, waitFor } from "@testing-library/react";
import { App } from "../src/App";
import { FixtureBackend } from "../src/bridge/fixture/FixtureBackend";
import { memoryHistory } from "../src/router";

/** Fixture backend with no timers and no latency: deterministic. */
export const quietBackend = (o: ConstructorParameters<typeof FixtureBackend>[0] = {}) =>
  new FixtureBackend({ live: false, latencyMs: 0, speed: 0, ...o });

export async function renderApp(path = "/inbox", backend = quietBackend()) {
  localStorage.clear();
  const history = memoryHistory(path);
  const utils = render(<App backend={backend} history={history} />);
  await waitFor(() => {
    if (!document.querySelector("[data-session]") && !path.startsWith("/session")) throw new Error("rows not loaded yet");
  });
  return { ...utils, backend, history };
}

export const sessionByName = (backend: FixtureBackend, name: string) => {
  for (const h of backend.hostIds()) {
    for (const id of backend.sessionIds(h)) {
      const snap = backend.snapshot(h, id);
      if (snap.title === name) return { host: h, id, key: `${h}/${id}`, snap };
    }
  }
  throw new Error(`no fixture session ${name}`);
};
