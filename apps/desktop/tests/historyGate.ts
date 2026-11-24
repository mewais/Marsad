import type { FixtureBackend } from "../src/bridge/fixture/FixtureBackend";
import type { HistoryPage, HistoryRequest, SessionEvent } from "../src/bridge/wire";

/**
 * Deferred `session.history` replies for a fixture backend: the first `pass` requests are answered at once, every later one is held
 * until the test releases it. Lets a test step the app's paging one page at a time and look at the store in between, instead of
 * waiting on timers. Held requests are answered in the order they were asked for.
 */
export interface HistoryGate {
  /** The requests held and not yet released, oldest first. */
  pending(): HistoryRequest[];
  /** Every request answered so far (passed through or released), in the order it was answered, with its page. */
  answered(): { params: HistoryRequest; page: HistoryPage }[];
  /** Answer the oldest held request, and wait until its reply has been handed to the app. */
  release(): Promise<void>;
  /** Stop holding: answer every held request now, and every later one as it comes. */
  open(): Promise<void>;
}

type Send = (method: string, params?: unknown, opts?: object) => Promise<unknown>;

export function holdHistory(b: FixtureBackend, pass = 0): HistoryGate {
  const request = b.request.bind(b) as unknown as Send;
  const held: { params: HistoryRequest; go: () => Promise<void> }[] = [];
  const answered: { params: HistoryRequest; page: HistoryPage }[] = [];
  let seen = 0;
  let opened = false;

  const answer = (params: HistoryRequest, method: string, p: unknown, opts?: object) =>
    request(method, p, opts).then((page) => {
      answered.push({ params, page: page as HistoryPage });
      return page;
    });

  (b as unknown as { request: Send }).request = (method, p, opts) => {
    if (method !== "session.history") return request(method, p, opts);
    const params = p as HistoryRequest;
    seen += 1;
    if (opened || seen <= pass) return answer(params, method, p, opts);
    return new Promise((resolve, reject) => {
      held.push({ params, go: () => answer(params, method, p, opts).then(resolve, reject).then(() => undefined) });
    });
  };

  const release = async () => {
    const next = held.shift();
    if (!next) throw new Error("holdHistory: no session.history request is held");
    await next.go();
  };

  return {
    pending: () => held.map((h) => h.params),
    answered: () => [...answered],
    release,
    async open() {
      opened = true;
      while (held.length) await release();
    },
  };
}

/** The fixture's journal of a session: every event it holds, oldest first (the fixture's own state, read for expectations only). */
export function journal(b: FixtureBackend, host: string, id: string): SessionEvent[] {
  return (b as unknown as { hosts: Map<string, { sessions: Map<string, { events: SessionEvent[] }> }> }).hosts.get(host)!.sessions.get(id)!.events;
}

/** The cursors of the events of a session's main scope that pass `keep`, oldest first: what a transcript of that scope holds once it has all of them. */
export function cursorsOf(events: readonly SessionEvent[], keep: (e: SessionEvent) => boolean): number[] {
  return events.filter(keep).map((e) => e.cursor);
}
