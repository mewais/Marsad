/**
 * Window background: solid, or translucent (independent of the theme). The choice lives in the
 * hub's settings (`surface`, `surface.strength`) and is cached on this device so the first
 * paint already has it. The CSS side is `[data-surface="translucent"]` in tokens.css (alpha
 * variants of the surface tokens, scaled by `--glass`); the native backdrop is the desktop
 * shell's `desktop.window_surface`.
 */
import { createStore } from "./store";

export type SurfaceChoice = "solid" | "translucent";
export const DEFAULT_STRENGTH = 50;
const KEY = "marsad.surface";
const KEY_STRENGTH = "marsad.surface.strength";

/** What the window can show, as the desktop shell answered (`null` = not asked yet). */
export interface SurfaceSupport {
  transparent: boolean;
  effect: string | null;
  /** Something the user can change for the effect to show (from the shell). */
  hint?: string | null;
}

/** The shell's latest answer for this window. */
export const surfaceSupport = createStore<SurfaceSupport | null>(null);

export function loadSurface(): { surface: SurfaceChoice; strength: number } {
  try {
    const s = localStorage.getItem(KEY);
    const n = Number(localStorage.getItem(KEY_STRENGTH));
    return { surface: s === "translucent" ? "translucent" : "solid", strength: clampStrength(localStorage.getItem(KEY_STRENGTH) === null ? DEFAULT_STRENGTH : n) };
  } catch {
    return { surface: "solid", strength: DEFAULT_STRENGTH };
  }
}

export function clampStrength(n: unknown): number {
  const v = typeof n === "number" && Number.isFinite(n) ? n : DEFAULT_STRENGTH;
  return Math.min(100, Math.max(0, Math.round(v)));
}

/**
 * Paint the choice. `support` is the shell's answer: when the window cannot be transparent
 * (no compositor), the UI stays solid whatever was picked.
 */
export function applySurface(surface: SurfaceChoice, strength: number, support: SurfaceSupport | null = null) {
  const el = document.documentElement;
  const on = surface === "translucent" && (support?.transparent ?? true);
  if (on) {
    el.setAttribute("data-surface", "translucent");
    // Whether the OS blurs what is behind decides how transparent the window layer may get.
    el.setAttribute("data-backdrop", support?.effect ?? "none");
    el.style.setProperty("--glass", String(clampStrength(strength) / 100));
  } else {
    el.removeAttribute("data-surface");
    el.removeAttribute("data-backdrop");
    el.style.removeProperty("--glass");
  }
  try {
    localStorage.setItem(KEY, surface);
    localStorage.setItem(KEY_STRENGTH, String(clampStrength(strength)));
  } catch {
    /* storage unavailable: the hub setting still applies once it arrives */
  }
}
