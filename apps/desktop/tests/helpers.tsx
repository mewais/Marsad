import { render, waitFor } from "@testing-library/react";
import { App } from "../src/App";
import { FixtureBackend } from "../src/bridge/fixture/FixtureBackend";
import { memoryHistory } from "../src/router";

/** Fixture backend with no timers and no latency: deterministic. */
export const quietBackend = (o: ConstructorParameters<typeof FixtureBackend>[0] = {}) =>
  new FixtureBackend({ live: false, latencyMs: 0, speed: 0, ...o });

/** Mount the app without waiting for anything to load (tests of what shows before it has). */
export function mountApp(path = "/inbox", backend = quietBackend()) {
  localStorage.clear();
  const history = memoryHistory(path);
  const utils = render(<App backend={backend} history={history} />);
  return { ...utils, backend, history };
}

export async function renderApp(path = "/inbox", backend = quietBackend()) {
  const mounted = mountApp(path, backend);
  await waitFor(() => {
    if (!document.querySelector("[data-session]") && !path.startsWith("/session")) throw new Error("rows not loaded yet");
  });
  return mounted;
}

/** Answer calls of `method` only when told to (or with an error), everything else as the fixture does. */
export function hold(backend: FixtureBackend, method: string) {
  const request = backend.request.bind(backend);
  const waiting: { go: () => void; fail: (e: unknown) => void }[] = [];
  let failWith: unknown;
  backend.request = (<T,>(m: string, params?: unknown, opts?: object) => {
    if (m !== method) return request<T>(m, params, opts);
    if (failWith !== undefined) return Promise.reject(failWith);
    return new Promise<T>((resolve, reject) => waiting.push({ go: () => request<T>(m, params, opts).then(resolve, reject), fail: reject }));
  }) as typeof backend.request;
  return {
    pending: () => waiting.length,
    release: () => waiting.splice(0).forEach((w) => w.go()),
    reject: (e: unknown) => waiting.splice(0).forEach((w) => w.fail(e)),
    /** From now on every call fails at once. */
    failAll: (e: unknown) => { failWith = e; },
    healAll: () => { failWith = undefined; },
  };
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
