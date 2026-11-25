import { useEffect, useState } from "react";
import { errorMessage } from "../../bridge/api";
import type { SearchHit, SearchPart } from "../../bridge/wire";
import { clockTime } from "../../lib/format";
import { useApi } from "../../state";
import { streamPages } from "../../state/find";
import type { TranscriptScope } from "../../state/scope";
import { IconButton, useEscape } from "../../ui";
import "./find.css";

/** How much of a prompt the outline shows. */
const SHOWN = 80;

/**
 * The outline of a transcript (B7b): its prompts, oldest first, each with its time and its first characters. A prompt is found with
 * `session.search` over the `user_text` field, page by page, so prompts beyond the loaded window are listed too; `onJump` takes the
 * reader to one (the same path as a find hit).
 */
export function Outline({ host, id, scope, parts, onJump, onClose }: {
  host: string;
  id: string;
  scope: TranscriptScope;
  /** The parts of the log to list, in the order given (the earlier history first, then the session's own events). */
  parts: readonly SearchPart[];
  onJump: (hit: SearchHit) => void;
  onClose: () => void;
}) {
  const api = useApi();
  const [state, setState] = useState<{ hits: SearchHit[]; done: boolean; error?: string }>({ hits: [], done: false });
  const key = `${host}\u0000${id}\u0000${JSON.stringify(scope)}\u0000${parts.join(",")}`;
  useEffect(() => {
    let live = true;
    setState({ hits: [], done: false });
    const request = (part: SearchPart, resume?: string) => ({ session: id, query: "", kinds: ["user_text"], scope, direction: "forward" as const, part, limit: 50, ...(resume ? { resume } : {}) });
    void (async () => {
      try {
        for (const part of parts) {
          await streamPages(
            () => api.search(host, request(part)),
            (token) => api.search(host, request(part, token)),
            () => live,
            (page) => setState((s) => ({ ...s, hits: [...s.hits, ...page.hits] })),
          );
          if (!live) return;
        }
        if (live) setState((s) => ({ ...s, done: true }));
      } catch (e) {
        if (live) setState((s) => ({ ...s, done: true, error: errorMessage(e) }));
      }
    })();
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `key` names every input the listing reads.
  }, [key]);
  useEscape(true, onClose);

  return (
    <aside className="tr-outline" aria-label="Outline">
      <div className="tr-outline-head">
        <span className="tr-outline-title">Prompts{state.done ? ` · ${state.hits.length}` : ""}</span>
        <IconButton icon="x" label="Close outline" onClick={onClose} />
      </div>
      {state.error ? <div className="c-failed tr-outline-error" role="alert">{state.error}</div> : null}
      <ol className="tr-outline-list">
        {state.hits.map((h, i) => (
          <li key={`${h.part}:${h.cursor}:${i}`}>
            <button type="button" className="tr-outline-item" onClick={() => onJump(h)}>
              <time className="tr-outline-time">{clockTime(h.at)}</time>
              <span className="tr-outline-text">{h.snippet.slice(0, SHOWN)}</span>
            </button>
          </li>
        ))}
      </ol>
      {!state.done && !state.error ? <div className="faint fs12 tr-outline-foot">Loading prompts…</div>
        : state.done && !state.error && state.hits.length === 0 ? <div className="faint fs12 tr-outline-foot">No prompts yet</div> : null}
    </aside>
  );
}
