import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type RefObject } from "react";
import { useApp } from "../../state";
import { mayStepBack, mayStepForward, newer, older, promptsOf, type Browse } from "./promptHistory";

/**
 * Up / Down in the message box walk through the session's earlier prompts (see `promptHistory.ts`). Up
 * only at the very start of the text, Down only at the very end, so editing several lines keeps working
 * (a recalled one-line prompt may be walked from anywhere, see `mayStepBack`).
 * The unsent draft is stashed on the first step back and restored when the walk comes back to it; any
 * edit ends the walk and makes the text the draft. Prompts older than what the transcript has loaded are
 * paged in on demand, in the background: the box stays editable meanwhile.
 */
export function usePromptRecall(sessionKey: string, text: string, setText: (t: string) => void, box: RefObject<HTMLTextAreaElement | null>) {
  const { store, transcripts } = useApp();
  const [browse, setBrowse] = useState<Browse | null>(null);
  const [paging, setPaging] = useState(false);
  /** Where the caret goes once the recalled text is in the box: the start after Up, the end after Down. */
  const caret = useRef<"start" | "end" | null>(null);
  /** Bumped to cancel a page-back in flight (the user edited, moved on or left). */
  const walk = useRef(0);

  const reset = () => {
    walk.current++;
    setBrowse(null);
    setPaging(false);
  };
  useEffect(() => {
    reset();
    return () => {
      walk.current++;
    };
  }, [sessionKey]);
  useLayoutEffect(() => {
    const el = box.current;
    if (!caret.current || !el) return;
    const at = caret.current === "start" ? 0 : el.value.length;
    el.setSelectionRange(at, at);
    caret.current = null;
  }, [text, browse, box]);

  const prompts = () => promptsOf(store.get().transcripts[sessionKey]?.events ?? []);
  const show = (to: Browse | null, value: string, where: "start" | "end") => {
    caret.current = where;
    setBrowse(to);
    setText(value);
  };

  async function pageBack(from: Browse | null, draft: string) {
    const mine = ++walk.current;
    setPaging(true);
    try {
      for (;;) {
        const slice = store.get().transcripts[sessionKey];
        if (!slice?.hasMore) return;
        const had = slice.events.length;
        await transcripts.loadEarlier(sessionKey);
        if (walk.current !== mine) return;
        const step = older(prompts(), from, draft);
        if (step !== "end") return show(step.browse, step.text, "start");
        if ((store.get().transcripts[sessionKey]?.events.length ?? had) === had) return; // nothing came: stop asking
      }
    } finally {
      if (walk.current === mine) setPaging(false);
    }
  }

  /** Handle Up / Down for the box; true when the key was used. */
  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>): boolean => {
    if (e.nativeEvent.isComposing || e.shiftKey || e.altKey || e.ctrlKey || e.metaKey) return false;
    const el = e.currentTarget;
    if (e.key === "ArrowUp" && mayStepBack(el, browse !== null)) {
      if (paging) {
        e.preventDefault();
        return true;
      }
      const step = older(prompts(), browse, text);
      if (step !== "end") {
        walk.current++;
        show(step.browse, step.text, "start");
      } else if (store.get().transcripts[sessionKey]?.hasMore) void pageBack(browse, browse ? browse.draft : text);
      else return false;
      e.preventDefault();
      return true;
    }
    if (e.key === "ArrowDown" && browse && mayStepForward(el, true)) {
      walk.current++;
      setPaging(false);
      const step = newer(prompts(), browse);
      show(step.browse, step.text, "end");
      e.preventDefault();
      return true;
    }
    return false;
  };

  return { browsing: browse !== null, paging, onKeyDown, reset };
}
