import { useEffect, useState } from "react";
import { errorMessage } from "../../bridge/api";
import type { SearchHit, SearchPart } from "../../bridge/wire";
import { useApi } from "../../state";
import { streamPages } from "../../state/find";
import type { TranscriptScope } from "../../state/scope";

export interface FindResult {
  /** The hits so far, in the order they arrived: the session's own events newest first, then the earlier history newest first. */
  hits: SearchHit[];
  /** Whether every page has arrived (false while the search runs). */
  done: boolean;
  error?: string;
}

/**
 * The hits of `query` in a transcript, streamed page by page. Each part of the log is searched backward from its newest event (`parts`
 * in the order given). A new query, or a new transcript, ends the search it replaces: its pages are dropped when they arrive.
 */
export function useFind(host: string, id: string, scope: TranscriptScope, parts: readonly SearchPart[], query: string): FindResult {
  const api = useApi();
  const [result, setResult] = useState<FindResult & { key: string }>({ key: "", hits: [], done: true });
  // The search runs for one set of inputs; its identity is the key (the scope is an object, so its value is what counts).
  const key = `${host}\u0000${id}\u0000${JSON.stringify(scope)}\u0000${parts.join(",")}\u0000${query}`;
  useEffect(() => {
    if (!query) {
      setResult({ key, hits: [], done: true });
      return;
    }
    let live = true;
    setResult({ key, hits: [], done: false });
    const request = (part: SearchPart, resume?: string) => ({ session: id, query, scope, direction: "backward" as const, part, limit: 50, ...(resume ? { resume } : {}) });
    void (async () => {
      try {
        for (const part of parts) {
          await streamPages(
            () => api.search(host, request(part)),
            (token) => api.search(host, request(part, token)),
            () => live,
            (page) => setResult((r) => ({ ...r, hits: [...r.hits, ...page.hits] })),
          );
          if (!live) return;
        }
        if (live) setResult((r) => ({ ...r, done: true }));
      } catch (e) {
        if (live) setResult((r) => ({ ...r, done: true, error: errorMessage(e) }));
      }
    })();
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `key` names every input the search reads.
  }, [key]);
  // What the search found for these inputs only: hits of a query that was replaced are never shown.
  return result.key === key ? result : { hits: [], done: query === "" };
}
