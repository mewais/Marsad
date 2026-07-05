/**
 * AppShell — the Workstreams-redesign app shell (SLICE A).
 *
 * A single 52px glass header (breadcrumbs, "+ New workstream", settings,
 * theme toggle) sitting over a fixed wallpaper layer, with the Board /
 * Graph / Session screen filling the rest. NO left sidebar, NO Overview
 * home, NO Workspaces switcher — those belonged to the old shell and are
 * gone. Ported pixel-close from "Marsad Workstreams.dc.html" lines 48-72.
 */

import React from "react";
import { useVm } from "../bridge/useVm";
import { WaslGateModal } from "../screens/WaslGateModal";
import { useTheme } from "../theme/ThemeProvider";
import { wallpaperGradient } from "../theme/tokens";
import { Watchtower } from "./Watchtower";

// ---------------------------------------------------------------------------
// Breadcrumb trail
// ---------------------------------------------------------------------------

function Breadcrumbs() {
  const { crumbs } = useVm();
  if (crumbs.length === 0) return null;
  return (
    <>
      {crumbs.map((c, i) => {
        const isLast = i === crumbs.length - 1;
        const clickable = c.onClick != null;
        return (
          <React.Fragment key={`${i}-${c.label}`}>
            <span style={{ color: "var(--txt3)" }}>/</span>
            <span
              data-testid={`crumb-${i}`}
              onClick={c.onClick ?? undefined}
              style={{
                fontWeight: 600,
                fontSize: "calc(13px * var(--fs,1))",
                // Inert (current-level) crumbs read at --txt2; a clickable
                // ancestor crumb (e.g. the parent workstream, from a session)
                // reads at --txt3 — matches the design's crumbs colouring.
                color: isLast || !clickable ? "var(--txt2)" : "var(--txt3)",
                cursor: clickable ? "pointer" : "default",
                whiteSpace: "nowrap",
                overflow: "hidden",
                textOverflow: "ellipsis",
                maxWidth: 220,
              }}
            >
              {c.label}
            </span>
          </React.Fragment>
        );
      })}
    </>
  );
}

// ---------------------------------------------------------------------------
// AppShell
// ---------------------------------------------------------------------------

interface AppShellProps {
  children: React.ReactNode;
  onNewSession?: () => void;
  onOpenSettings?: () => void;
}

export function AppShell({ children, onOpenSettings }: AppShellProps) {
  const { dispatch, openBoard, openNewWorkstream } = useVm();
  const { theme, toggleTheme } = useTheme();

  const themeIcon = theme === "dark" ? "☀" : "☾";

  return (
    <div
      className="appshell"
      style={{
        height: "100vh",
        display: "flex",
        flexDirection: "column",
        // Transparent so the OS-translucent window (tauri `transparent: true`) shows
        // through — the frosted tint comes from <body> (see index.html). Painting an
        // opaque `var(--bg)` here is what previously defeated translucency entirely.
        background: "transparent",
        color: "var(--txt)",
        overflow: "hidden",
        fontSize: "calc(16px * var(--fs,1))",
        position: "relative",
        zIndex: 1,
      }}
    >
      {/* ===== WALLPAPER — soft multi-radial-gradient, fixed behind everything ===== */}
      <div
        data-testid="wallpaper"
        aria-hidden="true"
        style={{
          position: "absolute",
          inset: 0,
          zIndex: 0,
          pointerEvents: "none",
          background: wallpaperGradient,
        }}
      />

      {/* ===== HEADER (glass) ===== */}
      <header
        style={{
          position: "relative",
          zIndex: 5,
          display: "flex",
          alignItems: "center",
          gap: 14,
          height: 52,
          padding: "0 18px",
          borderBottom: "1px solid var(--border)",
          background: "rgba(18,22,29,0.72)",
          backdropFilter: "blur(22px)",
          WebkitBackdropFilter: "blur(22px)",
          flexShrink: 0,
        }}
      >
        {/* Logo + wordmark — click → board home */}
        <div
          onClick={openBoard}
          style={{
            display: "flex",
            alignItems: "center",
            gap: 10,
            cursor: "pointer",
            flexShrink: 0,
          }}
          data-testid="wordmark"
        >
          <div
            style={{
              width: 26,
              height: 26,
              borderRadius: 7,
              background: "linear-gradient(135deg,var(--accent),var(--accent2))",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontWeight: 800,
              color: "#fff",
              fontSize: "calc(15px * var(--fs,1))",
            }}
          >
            <Watchtower size={15} />
          </div>
          <div style={{ display: "flex", flexDirection: "column", lineHeight: 1.05 }}>
            <span style={{ fontWeight: 700, fontSize: "calc(14.5px * var(--fs,1))", letterSpacing: -0.2 }}>
              Marsad <span style={{ color: "var(--txt3)", fontWeight: 400 }}>—</span> مرصد
            </span>
            <span
              style={{
                fontSize: "calc(9px * var(--fs,1))",
                color: "var(--txt3)",
                letterSpacing: 1.5,
                textTransform: "uppercase",
              }}
            >
              Watchtower
            </span>
          </div>
        </div>

        {/* Breadcrumbs: Marsad / <Workstream> / <Session> */}
        <Breadcrumbs />

        {/* Spacer */}
        <div style={{ flex: 1 }} />

        {/* New workstream — opens the New-workstream modal (SLICE E): name +
            goal, then the Single-session / Build-it-out path choice. */}
        <button
          data-testid="new-workstream-btn"
          onClick={openNewWorkstream}
          style={{
            fontFamily: "inherit",
            display: "flex",
            alignItems: "center",
            gap: 7,
            border: "none",
            background: "var(--accent)",
            color: "#fff",
            fontWeight: 700,
            fontSize: "calc(12px * var(--fs,1))",
            padding: "7px 13px",
            borderRadius: 9,
            cursor: "pointer",
          }}
        >
          ＋ New workstream
        </button>

        {/* Settings */}
        <button
          data-testid="open-settings-btn"
          onClick={onOpenSettings ?? (() => dispatch({ type: "SET_SCREEN", screen: "settings" }))}
          title="Settings"
          style={{
            width: 32,
            height: 32,
            borderRadius: 8,
            border: "1px solid var(--border)",
            background: "var(--panel2)",
            color: "var(--txt2)",
            cursor: "pointer",
            fontSize: "calc(15px * var(--fs,1))",
          }}
        >
          ⚙
        </button>

        {/* Theme toggle */}
        <button
          onClick={toggleTheme}
          title="Toggle theme"
          style={{
            width: 32,
            height: 32,
            borderRadius: 8,
            border: "1px solid var(--border)",
            background: "var(--panel2)",
            color: "var(--txt2)",
            cursor: "pointer",
            fontSize: "calc(14px * var(--fs,1))",
          }}
        >
          {themeIcon}
        </button>
      </header>

      {/* ===== BODY — Board / Graph / Session fills the rest ===== */}
      <main style={{ position: "relative", zIndex: 2, flex: 1, overflowY: "auto" }}>
        {children}
      </main>

      {/* Wasl handoff-approval modal — global overlay, surfaces on any screen
          when a Wasl node has a pending gate; renders nothing otherwise. */}
      <WaslGateModal />
    </div>
  );
}
