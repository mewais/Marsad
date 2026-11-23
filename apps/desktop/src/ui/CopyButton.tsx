import { useCallback, useEffect, useRef, useState } from "react";
import { copyText } from "../lib/clipboard";
import { IconButton } from "./Button";
import { toast } from "./toast";

export type CopyState = "idle" | "copied" | "failed";

/**
 * The one copy interaction. `state` is "copied" only after the clipboard write has resolved and
 * "failed" (plus an error toast with the reason) when it did not; both fall back to "idle" after a
 * moment. Used by CopyButton and by anything that copies on its own affordance (inline code).
 */
export function useCopy(text: string): { state: CopyState; copy: () => void } {
  const [state, setState] = useState<CopyState>("idle");
  const timer = useRef<ReturnType<typeof setTimeout>>();
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      clearTimeout(timer.current);
    };
  }, []);
  const copy = useCallback(() => {
    clearTimeout(timer.current);
    copyText(text).then(
      () => "copied" as const,
      (e: unknown) => {
        toast({ tone: "err", title: "Could not copy", detail: e instanceof Error ? e.message : String(e), ttl: 6000 });
        return "failed" as const;
      },
    ).then((next) => {
      if (!alive.current) return;
      setState(next);
      timer.current = setTimeout(() => setState("idle"), next === "copied" ? 1500 : 4000);
    });
  }, [text]);
  return { state, copy };
}

/** The accessible name of a copy control in each state. */
export const copyName = (state: CopyState, label: string) => (state === "copied" ? "Copied" : state === "failed" ? "Copy failed" : label);

export interface CopyButtonProps {
  /** Exactly what lands on the clipboard (source text, not rendered output). */
  text: string;
  /** Accessible name while idle, e.g. "Copy prompt". */
  label: string;
  className?: string;
}

/** Icon button that copies `text`; shows a check only after success and a warning after failure. */
export function CopyButton({ text, label, className = "" }: CopyButtonProps) {
  const { state, copy } = useCopy(text);
  const name = copyName(state, label);
  return (
    <>
      <IconButton icon={state === "copied" ? "check" : state === "failed" ? "warn" : "copy"} label={name} className={`copybtn ${className}`} data-state={state} onClick={copy} />
      {state === "idle" ? null : <span className="sr-only" role="status">{name}</span>}
    </>
  );
}
