/**
 * Design tokens — ported exactly from the :root CSS vars in Marsad.dc.html.
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
}

export interface Tokens extends ColorTokens {
  fontSans: string;
  fontMono: string;
}

/** Dark theme (default). Matches :root in Marsad.dc.html verbatim. */
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
  shadow: "0 12px 40px rgba(0,0,0,0.45)",
  shadowSm: "0 2px 10px rgba(0,0,0,0.3)",
  glassChrome: "rgba(18,22,29,0.55)",
  glassMain: "rgba(11,14,19,0.42)",
  fontSans: "'Hanken Grotesk', 'Marsad Symbols', system-ui, sans-serif",
  fontMono: "'JetBrains Mono', 'Marsad Symbols', monospace",
};

/** Light theme overrides. Colors adapted for readability on light backgrounds. */
export const lightTokens: Tokens = {
  bg: "#f0f2f5",
  panel: "#ffffff",
  panel2: "#f5f7fa",
  elev: "#ffffff",
  border: "#dde1e9",
  border2: "#c8cdd8",
  txt: "#1a2030",
  txt2: "#4a5568",
  txt3: "#8a95a5",
  accent: "#3b6fd4",
  accent2: "#5a8ce0",
  needs: "#c97b10",
  error: "#c0392b",
  done: "#2e9e6e",
  neutral: "#6e7885",
  needsBg: "rgba(201,123,16,0.10)",
  errorBg: "rgba(192,57,43,0.09)",
  workingBg: "rgba(59,111,212,0.09)",
  doneBg: "rgba(46,158,110,0.09)",
  accentBg: "rgba(59,111,212,0.10)",
  shadow: "0 12px 40px rgba(0,0,0,0.12)",
  shadowSm: "0 2px 10px rgba(0,0,0,0.08)",
  glassChrome: "rgba(240,242,245,0.70)",
  glassMain: "rgba(245,247,250,0.55)",
  fontSans: "'Hanken Grotesk', 'Marsad Symbols', system-ui, sans-serif",
  fontMono: "'JetBrains Mono', 'Marsad Symbols', monospace",
};

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
    "--font-sans": t.fontSans,
    "--font-mono": t.fontMono,
  };
}
