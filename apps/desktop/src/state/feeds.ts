import type { Unsubscribe, SubscriptionEnd } from "../bridge/backend";
import { errorMessage } from "../bridge/api";
import type { HostId } from "../bridge/wire";
import type { AppStore } from "./model";

/** What a subscription's `start` is given: call it once if the subscription ends for good. */
export type OnEnd = (end: SubscriptionEnd) => void;

/**
 * The store's live subscriptions. A subscription that ends (the daemon refused its topic, or
 * closed it) is recorded under its name in `feedErrors`, so the UI can say that what it shows is
 * no longer live and why, and it is started again by `retry`, which the app calls when the hub
 * link comes back and when the user asks, or by `retryHost`, which the app calls when one host's
 * link comes back. Nothing restarts by itself on a timer: a topic that keeps being refused would
 * otherwise spin.
 */
export interface Feeds {
  /**
   * Open the subscription `start` makes under `name`. `host`: the subscription is that host's, so
   * `retryHost` starts it again when the host's link is back. Returns what closes it for good.
   */
  open(name: string, start: (onEnd: OnEnd) => Unsubscribe, host?: HostId): Unsubscribe;
  /** Start every ended subscription again (each goes through its own `start`, so it resumes where it left off). */
  retry(): void;
  /** Start again the ended subscriptions of `host` only; the others are not touched. */
  retryHost(host: HostId): void;
}

export function createFeeds(store: AppStore): Feeds {
  interface Rec {
    start: (onEnd: OnEnd) => Unsubscribe;
    off: Unsubscribe;
    ended: boolean;
    host?: HostId;
  }
  const feeds = new Map<string, Rec>();

  const setError = (name: string, message: string | null) =>
    store.set((s) => {
      if (message === null && !(name in s.feedErrors)) return s;
      const feedErrors = { ...s.feedErrors };
      if (message === null) delete feedErrors[name];
      else feedErrors[name] = message;
      return { ...s, feedErrors };
    });

  const run = (name: string, rec: Rec) => {
    rec.ended = false;
    rec.off = rec.start((end) => {
      if (feeds.get(name) !== rec) return;
      rec.ended = true;
      setError(name, end ? errorMessage(end) : "the daemon closed it");
    });
  };

  const restart = (name: string, rec: Rec) => {
    rec.off();
    setError(name, null);
    run(name, rec);
  };

  return {
    open(name, start, host) {
      feeds.get(name)?.off();
      const rec: Rec = { start, off: () => undefined, ended: false, host };
      feeds.set(name, rec);
      run(name, rec);
      return () => {
        if (feeds.get(name) !== rec) return;
        rec.off();
        feeds.delete(name);
        setError(name, null);
      };
    },
    retry() {
      for (const [name, rec] of [...feeds]) if (rec.ended) restart(name, rec);
    },
    retryHost(host) {
      for (const [name, rec] of [...feeds]) if (rec.ended && rec.host === host) restart(name, rec);
    },
  };
}
