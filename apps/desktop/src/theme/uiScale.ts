// Per-machine FONT scale. Font sizes render larger/smaller across systems at
// different DPI (e.g. WSLg vs native Windows), so we expose a single factor the
// user tunes per machine. Persisted in localStorage (a pure UI preference; no
// backend round-trip needed).
//
// This scales TYPOGRAPHY ONLY — not layout. Every inline `fontSize` in the UI is
// written as `calc(<px> * var(--fs,1))`, so setting `--fs` on :root multiplies all
// text (and the glyph-based icons, which inherit font-size) while paddings, widths,
// gaps and other px dimensions stay fixed. No `zoom`, so the shell never reflows or
// leaves gaps.
const KEY = "marsad.uiScale";

export function getUiScale(): number {
  const v = parseFloat(localStorage.getItem(KEY) ?? "");
  return Number.isFinite(v) && v >= 0.6 && v <= 1.8 ? v : 1.0;
}

export function applyUiScale(scale: number): void {
  const s = Math.min(1.8, Math.max(0.6, scale));
  localStorage.setItem(KEY, String(s));
  document.documentElement.style.setProperty("--fs", String(s));
}
