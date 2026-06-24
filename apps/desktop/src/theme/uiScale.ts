// Per-machine UI scale. Font sizes are in px (they render larger/smaller across
// systems at different DPI — e.g. WSLg vs native Windows), so we expose a single
// zoom factor the user tunes per machine. Persisted in localStorage (a pure UI
// preference; no backend round-trip needed).
const KEY = "marsad.uiScale";

export function getUiScale(): number {
  const v = parseFloat(localStorage.getItem(KEY) ?? "");
  return Number.isFinite(v) && v >= 0.6 && v <= 1.8 ? v : 1.0;
}

export function applyUiScale(scale: number): void {
  const s = Math.min(1.8, Math.max(0.6, scale));
  localStorage.setItem(KEY, String(s));
  // WebView2 (Windows) and WebKitGTK both honor CSS `zoom` and reflow correctly.
  (document.documentElement.style as CSSStyleDeclaration & { zoom?: string }).zoom =
    String(s);
}
