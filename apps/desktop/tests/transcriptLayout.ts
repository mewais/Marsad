import { act, fireEvent, waitFor } from "@testing-library/react";
import { expect } from "vitest";

/*
 * jsdom has no layout. A transcript (`.transcript`, one virtual list) with no viewport mounts only its last rows, so a row near the
 * top is not in the DOM until the transcript is scrolled there. These helpers give transcripts a viewport of `px` pixels (rows are
 * estimated at their estimate, since nothing is measured) and scroll them the way a reader does, so tests reach a row by scrolling.
 * A scroll step is larger than the viewport but smaller than the mounted window (viewport plus overscan), so no row is skipped.
 */

/** Gives every transcript a viewport of `px` pixels. Returns the function that removes the mock. */
export function layoutTranscript(px = 400): () => void {
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.classList.contains("transcript") ? px : 0;
    },
  });
  return () => {
    delete (HTMLElement.prototype as { clientHeight?: number }).clientHeight;
  };
}

export const transcript = (): HTMLElement => document.querySelector(".transcript") as HTMLElement;

/**
 * Scrolls the transcript to `top`, as a reader's scroll does: a wheel gesture precedes the move (so a list that follows the end
 * releases following, the way it does for a real reader and not for a programmatic reflow), the position is set, and the scroll
 * event fires.
 */
export function scrollTranscript(top: number) {
  const el = transcript();
  act(() => {
    fireEvent.wheel(el);
    el.scrollTop = top;
    fireEvent.scroll(el);
  });
}

/** Scrolls up (`dir` -1) or down (`dir` 1) a step at a time until `found()` holds. Returns whether it does. */
export function scrollUntil(found: () => boolean, dir: -1 | 1 = -1, step = 1000, maxSteps = 200): boolean {
  for (let i = 0; i < maxSteps && !found(); i++) {
    const el = transcript();
    const next = dir < 0 ? Math.max(0, el.scrollTop - step) : el.scrollTop + step;
    if (next === el.scrollTop) break;
    scrollTranscript(next);
  }
  return found();
}

/**
 * Scrolls until `find()` returns an element, and returns it. Without `dir` it looks up from where the transcript is, then down, since
 * a row may be on either side. Waits for the element to arrive (a load), scrolling each time.
 */
export async function reach<T extends Element>(find: () => T | null, dir?: -1 | 1): Promise<T> {
  let found: T | null = null;
  await waitFor(() => {
    for (const d of dir ? [dir] : [-1, 1] as const) if (scrollUntil(() => find() !== null, d)) break;
    found = find();
    expect(found).not.toBeNull();
  });
  return found as unknown as T;
}

export const toTop = () => scrollUntil(() => false, -1);
export const toBottom = () => scrollTranscript(Number.MAX_SAFE_INTEGER);
