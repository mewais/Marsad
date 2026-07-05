/**
 * Design tokens — ported exactly from the Design Tokens table in
 * docs/design/DESIGN_HANDOFF.md (and the :root CSS vars in
 * "Marsad Workstreams.dc.html", lines 16-24).
 * Dark mode is the baseline; light mode overrides are in lightTokens.
 */

export interface ColorTokens {
  bg: string;
  panel: string;
  panel2: string;
  elev: string;
  border: string;
  border2: string;
  txt: string;
  txt2: string;
  txt3: string;
  accent: string;
  accent2: string;
  needs: string;
  error: string;
  done: string;
  neutral: string;
  needsBg: string;
  errorBg: string;
  workingBg: string;
  doneBg: string;
  accentBg: string;
  shadow: string;
  shadowSm: string;
  glassChrome: string;
  glassMain: string;
  // Driver colours — identical in both themes (docs/design/DESIGN_HANDOFF.md
  // §Design Tokens > Driver colours). First-class, colour-coded everywhere.
  dClaude: string;
  dCodex: string;
  dGemini: string;
}

export interface Tokens extends ColorTokens {
  fontSans: string;
  fontMono: string;
}

// Driver colours are the same hex in both themes.
const DRIVER_COLORS = {
  dClaude: "#d97757",
  dCodex: "#3fb6a8",
  dGemini: "#9b8cf0",
};

/** Dark theme (default). Matches :root in "Marsad Workstreams.dc.html" verbatim. */
export const darkTokens: Tokens = {
  bg: "#0e1116",
  panel: "#151921",
  panel2: "#1a1f29",
  elev: "#1e2530",
  border: "#242b37",
  border2: "#313a48",
  txt: "#e8ecf1",
  txt2: "#97a1b1",
  txt3: "#5f6979",
  accent: "#5b8def",
  accent2: "#7aa2f5",
  needs: "#f1a83a",
  error: "#e0594a",
  done: "#46c08a",
  neutral: "#7e8896",
  needsBg: "rgba(241,168,58,0.13)",
  errorBg: "rgba(224,89,74,0.12)",
  workingBg: "rgba(91,141,239,0.12)",
  doneBg: "rgba(70,192,138,0.12)",
  accentBg: "rgba(91,141,239,0.14)",
  shadow: "0 14px 44px rgba(0,0,0,0.5)",
  shadowSm: "0 2px 10px rgba(0,0,0,0.32)",
  glassChrome: "rgba(18,22,29,0.55)",
  glassMain: "rgba(11,14,19,0.42)",
  ...DRIVER_COLORS,
  fontSans: "'Hanken Grotesk', 'Marsad Symbols', system-ui, sans-serif",
  fontMono: "'JetBrains Mono', 'Marsad Symbols', monospace",
};

/**
 * Light theme overrides — exact hex from DESIGN_HANDOFF.md §Colours — Light
 * theme. `accent`/`accent2` are NOT overridden there ("accent stays #5b8def")
 * so they're inherited from darkTokens verbatim.
 */
export const lightTokens: Tokens = {
  bg: "#eef1f6",
  panel: "#ffffff",
  panel2: "#f4f6fa",
  elev: "#ffffff",
  border: "#e2e7ee",
  border2: "#cfd6e0",
  txt: "#1a222e",
  txt2: "#5a6675",
  txt3: "#8a94a3",
  accent: darkTokens.accent,
  accent2: darkTokens.accent2,
  needs: "#c07d10",
  error: "#cf4436",
  done: "#1f9d6b",
  neutral: "#6e7885",
  needsBg: "rgba(192,125,16,0.12)",
  errorBg: "rgba(207,68,54,0.10)",
  workingBg: "rgba(63,111,214,0.10)",
  doneBg: "rgba(31,157,107,0.10)",
  accentBg: "rgba(63,111,214,0.10)",
  shadow: "0 14px 44px rgba(20,30,50,0.16)",
  shadowSm: "0 2px 10px rgba(20,30,50,0.09)",
  glassChrome: "rgba(240,242,245,0.70)",
  glassMain: "rgba(245,247,250,0.55)",
  ...DRIVER_COLORS,
  fontSans: "'Hanken Grotesk', 'Marsad Symbols', system-ui, sans-serif",
  fontMono: "'JetBrains Mono', 'Marsad Symbols', monospace",
};

/**
 * The wallpaper — a soft multi-radial-gradient (blue/violet/teal blooms)
 * sitting behind everything, ported EXACTLY from "Marsad Workstreams.dc.html"
 * line 49. Same in both themes (the prototype's themeVars override never
 * touches this layer).
 */
export const wallpaperGradient =
  "radial-gradient(900px 620px at 10% -6%, rgba(91,141,239,0.13), transparent 60%), " +
  "radial-gradient(820px 600px at 96% 8%, rgba(155,140,240,0.11), transparent 58%), " +
  "radial-gradient(760px 760px at 78% 108%, rgba(63,182,168,0.08), transparent 58%)";

/** Wallpaper background CSS for glass dark mode (from .glass .wallpaper in .dc.html). */
export const glassDarkWallpaper =
  "radial-gradient(1000px 720px at 8% -4%, rgba(91,141,239,0.30), transparent 58%), " +
  "radial-gradient(900px 650px at 96% 10%, rgba(167,139,250,0.24), transparent 55%), " +
  "radial-gradient(820px 820px at 80% 104%, rgba(70,192,138,0.17), transparent 55%), " +
  "linear-gradient(155deg, #080b10, #10141b 55%, #0a0d13)";

/** Wallpaper background CSS for glass light mode. */
export const glassLightWallpaper =
  "radial-gradient(1000px 720px at 8% -4%, rgba(91,141,239,0.28), transparent 58%), " +
  "radial-gradient(900px 650px at 96% 10%, rgba(167,139,250,0.24), transparent 55%), " +
  "radial-gradient(820px 820px at 80% 104%, rgba(70,192,138,0.20), transparent 55%), " +
  "linear-gradient(155deg, #e7ebf2, #eef1f6 55%, #e6eaf1)";

/** Map token object to CSS custom property declarations. */
export function tokensToCssVars(t: Tokens): Record<string, string> {
  return {
    "--bg": t.bg,
    "--panel": t.panel,
    "--panel2": t.panel2,
    "--elev": t.elev,
    "--border": t.border,
    "--border2": t.border2,
    "--txt": t.txt,
    "--txt2": t.txt2,
    "--txt3": t.txt3,
    "--accent": t.accent,
    "--accent2": t.accent2,
    "--needs": t.needs,
    "--error": t.error,
    "--done": t.done,
    "--neutral": t.neutral,
    "--needs-bg": t.needsBg,
    "--error-bg": t.errorBg,
    "--working-bg": t.workingBg,
    "--done-bg": t.doneBg,
    "--accent-bg": t.accentBg,
    "--shadow": t.shadow,
    "--shadow-sm": t.shadowSm,
    "--glass-chrome": t.glassChrome,
    "--glass-main": t.glassMain,
    "--d-claude": t.dClaude,
    "--d-codex": t.dCodex,
    "--d-gemini": t.dGemini,
    "--font-sans": t.fontSans,
    "--font-mono": t.fontMono,
  };
}
