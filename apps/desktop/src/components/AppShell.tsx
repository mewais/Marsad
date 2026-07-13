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
import type { SessionView, Host } from "../bridge/types";
import { connHintForHost } from "../bridge/connectivity";
import { WaslGateModal } from "../screens/WaslGateModal";
import { useTheme } from "../theme/ThemeProvider";
import { wallpaperGradient } from "../theme/tokens";
import { Watchtower } from "./Watchtower";

// ---------------------------------------------------------------------------
// Fleet status notifiers — compact per-state dot+count roll-up for the top bar.
// Same dot style/colors as the dashboard's status dots. Shown on graph/session
// screens (not the board, where it's redundant) so a single-session focus still
// surfaces the whole fleet's status.
// ---------------------------------------------------------------------------

function StatusNotifiers({
  sessions,
  hosts,
}: {
  sessions: SessionView[];
  hosts: Host[];
}) {
  // Bucket every session into the dashboard's four active buckets plus a neutral
  // "paused". Two-axis model — mirrors marsad_workstream::severity: `Done`
  // always counts as done (finished its job, shown regardless of liveness); any
  // other phase only counts as active while the session is LIVE (Resumed); a
  // not-live (Paused) session is paused regardless of its last phase.
  const counts = {
    working: 0,
    needs: 0,
    error: 0,
    done: 0,
    paused: 0,
    reconnecting: 0,
    disconnected: 0,
  };
  for (const s of sessions) {
    // Connectivity axis (orthogonal to lifecycle): a session whose host link is
    // (re)connecting or down is ALSO tallied here, in addition to its lifecycle
    // bucket — the two are independent (a Resumed+Working session can be
    // Reconnecting). Host-derived, mirrors the per-session hint.
    const hint = connHintForHost(hosts, s.host_id);
    if (hint === "Reconnecting") counts.reconnecting += 1;
    else if (hint === "Disconnected") counts.disconnected += 1;

    // Starting is folded into Done (idle): a launched-but-idle session is done,
    // not working.
    if (s.state === "Done" || s.state === "Starting") {
      counts.done += 1;
      continue;
    }
    if (s.live === false) {
      counts.paused += 1;
      continue;
    }
    switch (s.state) {
      case "Working":
        counts.working += 1;
        break;
      case "Needs":
        counts.needs += 1;
        break;
      case "Error":
        counts.error += 1;
        break;
      case "Stale":
      case "Detached":
        counts.paused += 1;
        break;
    }
  }
  // Fixed priority order; colors match the dashboard (headStateColor + neutral).
  // Connectivity (reconnecting/disconnected) trails the lifecycle buckets.
  const buckets = (
    [
      { key: "working", color: "var(--accent)", pulse: true, title: "working" },
      { key: "needs", color: "var(--needs)", pulse: true, title: "needs you" },
      { key: "error", color: "var(--error)", pulse: false, title: "error" },
      { key: "done", color: "var(--done)", pulse: false, title: "done" },
      { key: "paused", color: "var(--txt3)", pulse: false, title: "paused" },
      { key: "reconnecting", color: "var(--needs)", pulse: true, title: "reconnecting" },
      { key: "disconnected", color: "var(--error)", pulse: false, title: "disconnected" },
    ] satisfies { key: keyof typeof counts; color: string; pulse: boolean; title: string }[]
  ).filter((b) => counts[b.key] > 0);

  if (buckets.length === 0) return null;

  return (
    <div
      data-testid="fleet-status-notifiers"
      style={{ display: "flex", alignItems: "center", gap: 12, marginRight: 4 }}
    >
      {buckets.map((b) => (
        <span
          key={b.key}
          title={`${counts[b.key]} ${b.title}`}
          style={{ display: "inline-flex", alignItems: "center", gap: 5 }}
        >
          <span
            style={{
              width: 9,
              height: 9,
              borderRadius: "50%",
              background: b.color,
              display: "inline-block",
              flexShrink: 0,
              animation: b.pulse ? "dotpulse 1.4s infinite" : undefined,
            }}
          />
          <span style={{ fontSize: "calc(12px * var(--fs,1))", fontWeight: 700, color: "var(--txt2)" }}>
            {counts[b.key]}
          </span>
        </span>
      ))}
    </div>
  );
}

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
  const { state, dispatch, openBoard, openNewWorkstream } = useVm();
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
          <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
            <span style={{ fontWeight: 700, fontSize: "calc(14.5px * var(--fs,1))", letterSpacing: -0.2, lineHeight: 1.25 }}>
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

        {/* Fleet status notifiers — a compact roll-up of every session's state,
            so while focused on one session/workstream you still see the whole
            fleet. Hidden on the board (redundant there). Same dot style as the
            dashboard. */}
        {(state.screen === "graph" || state.screen === "session") && (
          <StatusNotifiers
            sessions={state.snapshot?.sessions ?? []}
            hosts={state.snapshot?.hosts ?? []}
          />
        )}

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
