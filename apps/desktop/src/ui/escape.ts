import { useEffect, useRef } from "react";

/**
 * One owner for the Escape key. Every overlay that wants Esc registers while it is open; one
 * capture listener hands the key to the innermost (most recently registered) entry only and
 * swallows it, so a menu inside a dialog closes alone and the dialog survives its first Esc.
 *
 * Precedence follows from who registers: popovers and selects open on top of dialogs, dialogs and
 * the command palette on top of the page, inline editors only while editing. Things that are not
 * overlays (the composer's blur, the Inbox filter, the page-level "back" shortcut) are not
 * registered: they see Esc only when the stack is empty.
 */
export type EscapeHandler = (e: KeyboardEvent) => void;

const stack: { handler: EscapeHandler }[] = [];

function onKeyDown(e: KeyboardEvent) {
  if (e.key !== "Escape" || e.isComposing) return;
  const top = stack[stack.length - 1];
  if (!top) return;
  e.preventDefault();
  e.stopPropagation();
  top.handler(e);
}

/** Register `handler` as the innermost Escape owner until the returned function is called. */
export function pushEscape(handler: EscapeHandler): () => void {
  const entry = { handler };
  if (stack.length === 0) document.addEventListener("keydown", onKeyDown, true);
  stack.push(entry);
  return () => {
    const i = stack.indexOf(entry);
    if (i < 0) return;
    stack.splice(i, 1);
    if (stack.length === 0) document.removeEventListener("keydown", onKeyDown, true);
  };
}

/**
 * Own Escape while `active`. Registered once per activation (the handler may change freely), so
 * re-renders never move an entry above something opened after it.
 */
export function useEscape(active: boolean, handler: EscapeHandler) {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => (active ? pushEscape((e) => ref.current(e)) : undefined), [active]);
}
