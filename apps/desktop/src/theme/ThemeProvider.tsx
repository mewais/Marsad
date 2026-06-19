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
import {
  darkTokens,
  glassDarkWallpaper,
  glassLightWallpaper,
  lightTokens,
  tokensToCssVars,
} from "./tokens";

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
  const [theme, setThemeState] = useState<Theme>(initialTheme);
  const [glass, setGlassState] = useState(initialGlass);

  // Inject CSS vars on :root whenever theme/glass changes.
  useEffect(() => {
    const tokens = theme === "dark" ? darkTokens : lightTokens;
    const vars = tokensToCssVars(tokens);
    const root = document.documentElement;
    for (const [key, val] of Object.entries(vars)) {
      root.style.setProperty(key, val);
    }

    // Glass class on <body> triggers glass-mode CSS (blur/backdrop-filter).
    if (glass) {
      document.body.classList.add("glass");
      if (theme === "light") {
        document.body.classList.add("lightwp");
      } else {
        document.body.classList.remove("lightwp");
      }
    } else {
      document.body.classList.remove("glass", "lightwp");
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
      {/* Wallpaper layer — only visible in glass mode (display toggled via CSS class) */}
      <div
        className="wallpaper"
        style={{
          position: "fixed",
          inset: 0,
          zIndex: 0,
          background:
            theme === "dark" ? glassDarkWallpaper : glassLightWallpaper,
          display: glass ? "block" : "none",
        }}
      />
      {children}
    </ThemeContext.Provider>
  );
}
