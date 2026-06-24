/**
 * ThemeProvider — injects CSS custom properties for the active theme + glass mode.
 * Exposes useTheme() hook for reading/toggling theme state.
 */

import React, {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
} from "react";
import { darkTokens, lightTokens, tokensToCssVars } from "./tokens";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type Theme = "dark" | "light";

export interface ThemeContextValue {
  theme: Theme;
  glass: boolean;
  toggleTheme: () => void;
  toggleGlass: () => void;
  setTheme: (t: Theme) => void;
  setGlass: (g: boolean) => void;
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

const ThemeContext = createContext<ThemeContextValue | null>(null);

export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error("useTheme must be used inside ThemeProvider");
  return ctx;
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

export interface ThemeProviderProps {
  children: React.ReactNode;
  initialTheme?: Theme;
  initialGlass?: boolean;
}

export function ThemeProvider({
  children,
  initialTheme = "dark",
  initialGlass = false,
}: ThemeProviderProps) {
  // Persist theme + translucency per machine so they survive relaunch.
  const [theme, setThemeState] = useState<Theme>(() => {
    const v = localStorage.getItem("marsad.theme");
    return v === "dark" || v === "light" ? v : initialTheme;
  });
  const [glass, setGlassState] = useState<boolean>(() => {
    const v = localStorage.getItem("marsad.glass");
    return v === null ? initialGlass : v === "true";
  });

  // Inject CSS vars on :root whenever theme/glass changes.
  useEffect(() => {
    localStorage.setItem("marsad.theme", theme);
    localStorage.setItem("marsad.glass", String(glass));
    const tokens = theme === "dark" ? darkTokens : lightTokens;
    const vars = tokensToCssVars(tokens);
    const root = document.documentElement;
    for (const [key, val] of Object.entries(vars)) {
      root.style.setProperty(key, val);
    }

    // Translucency toggle. ON → make the surface tokens translucent so the OS window
    // material (mica on Windows / desktop on a compositing Linux-macOS) shows through
    // the canvas AND the panels/sidebar/buttons (not just the empty background). OFF →
    // keep the opaque defaults (already set above) for a solid look.
    if (glass) {
      document.body.classList.add("glass");
      root.style.setProperty("--bg", "transparent");
      root.style.setProperty("--panel", tokens.glassChrome);
      root.style.setProperty("--panel2", tokens.glassMain);
      root.style.setProperty("--elev", tokens.glassChrome);
      document.body.style.background = "transparent";
    } else {
      document.body.classList.remove("glass");
      document.body.style.background = tokens.bg;
    }

    // Theme class for any non-var CSS.
    document.body.setAttribute("data-theme", theme);
  }, [theme, glass]);

  const setTheme = useCallback((t: Theme) => setThemeState(t), []);
  const setGlass = useCallback((g: boolean) => setGlassState(g), []);
  const toggleTheme = useCallback(
    () => setThemeState((t) => (t === "dark" ? "light" : "dark")),
    []
  );
  const toggleGlass = useCallback(() => setGlassState((g) => !g), []);

  return (
    <ThemeContext.Provider
      value={{ theme, glass, toggleTheme, toggleGlass, setTheme, setGlass }}
    >
      {children}
    </ThemeContext.Provider>
  );
}
