/**
 * AppShell — 54px header + 312px sidebar + main slot.
 * Faithfully ported from Marsad.dc.html chrome.
 */

import React, { useRef } from "react";
import type { Filter, HostStatus } from "../bridge/types";
import { useVm } from "../bridge/useVm";
import { useTheme } from "../theme/ThemeProvider";
import { Badge } from "./Badge";
import { StateDot } from "./StateDot";
import { Chip } from "./Chip";

// ---------------------------------------------------------------------------
// Colour helpers
// ---------------------------------------------------------------------------

function hostStatusColor(status: HostStatus): string {
  if (status === "Connected") return "var(--done)";
  if (status === "Reconnecting") return "var(--needs)";
  return "var(--error)";
}

function hostStatusLabel(status: HostStatus): string {
  if (status === "Connected") return "connected";
  if (status === "Reconnecting") return "reconnecting";
  return "unreachable";
}

// ---------------------------------------------------------------------------
// Sidebar session row
// ---------------------------------------------------------------------------

function SidebarRow({
  name,
  state,
  host_label,
  project,
  selected,
  onSelect,
}: {
  name: string;
  state: import("../bridge/types").Phase;
  host_label: string;
  project: string;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <div
      onClick={onSelect}
      style={{
        display: "flex",
        gap: 10,
        padding: 10,
        borderRadius: 9,
        cursor: "pointer",
        marginBottom: 3,
        border: selected ? "1px solid var(--accent)" : "1px solid transparent",
        background: selected ? "var(--working-bg)" : "transparent",
      }}
    >
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: 4,
          paddingTop: 2,
        }}
      >
        <StateDot state={state} size={8} />
      </div>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
          <span
            style={{
              fontWeight: 600,
              fontSize: 12.5,
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
              flex: 1,
            }}
          >
            {name}
          </span>
        </div>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 6,
            marginTop: 3,
          }}
        >
          <Badge state={state} />
          <span
            className="mono"
            style={{
              fontSize: 9.5,
              color: "var(--txt3)",
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
            }}
          >
            {host_label}
          </span>
        </div>
        <div
          className="mono"
          style={{
            fontSize: 9.5,
            color: "var(--txt3)",
            marginTop: 3,
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {project}
        </div>
      </div>
    </div>
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

export function AppShell({ children, onNewSession, onOpenSettings }: AppShellProps) {
  const { state, dispatch, counts, sidebarList, hosts, workspaces, activeWs } =
    useVm();
  const { theme, toggleTheme } = useTheme();
  const searchRef = useRef<HTMLInputElement>(null);

  const themeIcon = theme === "dark" ? "☀" : "☾";

  const filters: { label: string; value: Filter; count?: number }[] = [
    { label: "All", value: "All", count: counts.total },
    { label: "Needs you", value: "Needs", count: counts.needs },
    { label: "Working", value: "Working", count: counts.working },
    { label: "Errors", value: "Error", count: counts.errors },
  ];

  function goOverview() {
    dispatch({ type: "SET_SCREEN", screen: "overview" });
    dispatch({ type: "SELECT_SESSION", id: null });
  }

  return (
    <div
      className="appshell"
      style={{
        height: "100vh",
        display: "flex",
        flexDirection: "column",
        background: "var(--bg)",
        color: "var(--txt)",
        overflow: "hidden",
        fontSize: 13.5,
        position: "relative",
        zIndex: 1,
      }}
    >
      {/* ===== HEADER ===== */}
      <header
        style={{
          display: "flex",
          alignItems: "center",
          gap: 18,
          height: 54,
          padding: "0 18px",
          borderBottom: "1px solid var(--border)",
          background: "var(--panel)",
          flexShrink: 0,
          zIndex: 20,
        }}
      >
        {/* Wordmark */}
        <div
          onClick={goOverview}
          style={{
            display: "flex",
            alignItems: "center",
            gap: 10,
            cursor: "pointer",
          }}
          data-testid="wordmark"
        >
          <div
            style={{
              width: 26,
              height: 26,
              borderRadius: 8,
              background:
                "linear-gradient(135deg,var(--accent),var(--accent2))",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontWeight: 800,
              color: "#fff",
              fontSize: 15,
            }}
          >
            M
          </div>
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              lineHeight: 1.05,
            }}
          >
            <span
              style={{
                fontWeight: 700,
                fontSize: 15,
                letterSpacing: -0.2,
              }}
            >
              Marsad{" "}
              <span style={{ color: "var(--txt3)", fontWeight: 700 }}>—</span>{" "}
              مرصد
            </span>
            <span
              style={{
                fontSize: 9.5,
                color: "var(--txt3)",
                letterSpacing: 1.5,
                textTransform: "uppercase",
              }}
            >
              Observatory
            </span>
          </div>
        </div>

        {/* Workspace switcher pill */}
        <div style={{ position: "relative", flexShrink: 0 }}>
          <button
            onClick={() => dispatch({ type: "TOGGLE_WS_MENU" })}
            title="Workspace"
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              background: "var(--panel2)",
              border: "1px solid var(--border)",
              borderRadius: 9,
              padding: "6px 11px",
              cursor: "pointer",
              color: "var(--txt)",
              fontFamily: "inherit",
            }}
          >
            <span style={{ fontSize: 12, color: "var(--txt3)" }}>⛶</span>
            <span
              style={{
                fontSize: 12.5,
                fontWeight: 600,
                maxWidth: 160,
                whiteSpace: "nowrap",
                overflow: "hidden",
                textOverflow: "ellipsis",
              }}
            >
              {activeWs?.name ?? "Default"}
            </span>
            <span
              style={{
                width: 6,
                height: 6,
                borderRadius: "50%",
                background: "var(--done)",
              }}
              title="Auto-saved"
            />
            <span style={{ fontSize: 9, color: "var(--txt3)" }}>▾</span>
          </button>

          {state.wsMenuOpen && (
            <>
              <div
                onClick={() => dispatch({ type: "CLOSE_WS_MENU" })}
                style={{ position: "fixed", inset: 0, zIndex: 54 }}
              />
              <div
                style={{
                  position: "absolute",
                  top: "calc(100% + 7px)",
                  left: 0,
                  width: 320,
                  background: "var(--elev)",
                  border: "1px solid var(--border2)",
                  borderRadius: 13,
                  boxShadow: "var(--shadow)",
                  zIndex: 55,
                  overflow: "hidden",
                  animation: "slideup .15s ease",
                }}
              >
                <div
                  style={{
                    padding: "13px 15px",
                    borderBottom: "1px solid var(--border)",
                  }}
                >
                  <div
                    style={{
                      fontSize: 9.5,
                      fontWeight: 700,
                      letterSpacing: 0.6,
                      textTransform: "uppercase",
                      color: "var(--txt3)",
                    }}
                  >
                    Current workspace
                  </div>
                  <div
                    style={{
                      display: "flex",
                      alignItems: "center",
                      gap: 8,
                      marginTop: 5,
                    }}
                  >
                    <span style={{ fontWeight: 700, fontSize: 14 }}>
                      {activeWs?.name ?? "Default"}
                    </span>
                    <span style={{ fontSize: 10, color: "var(--done)" }}>
                      ● auto-saved
                    </span>
                  </div>
                </div>
                <div style={{ padding: "8px 8px 4px" }}>
                  <div
                    style={{
                      fontSize: 9.5,
                      fontWeight: 700,
                      letterSpacing: 0.6,
                      textTransform: "uppercase",
                      color: "var(--txt3)",
                      padding: "4px 7px",
                    }}
                  >
                    Switch to
                  </div>
                  {workspaces
                    .filter((w) => w.id !== state.snapshot?.active_ws)
                    .map((ws) => (
                      <div
                        key={ws.id}
                        onClick={() => {
                          dispatch({ type: "SET_WORKSPACE", id: ws.id });
                        }}
                        style={{
                          display: "flex",
                          alignItems: "center",
                          gap: 10,
                          padding: "8px 9px",
                          borderRadius: 8,
                          cursor: "pointer",
                        }}
                      >
                        <div style={{ flex: 1, minWidth: 0 }}>
                          <div
                            style={{
                              fontSize: 12.5,
                              fontWeight: 600,
                              whiteSpace: "nowrap",
                              overflow: "hidden",
                              textOverflow: "ellipsis",
                            }}
                          >
                            {ws.name}
                          </div>
                          <div
                            className="mono"
                            style={{ fontSize: 9.5, color: "var(--txt3)" }}
                          >
                            {ws.session_ids.length} sessions · saved {ws.saved_at}
                          </div>
                        </div>
                      </div>
                    ))}
                </div>
                <div
                  style={{
                    padding: "7px 8px",
                    borderTop: "1px solid var(--border)",
                    display: "flex",
                    flexDirection: "column",
                    gap: 1,
                  }}
                >
                  <button
                    onClick={() => {
                      dispatch({ type: "CLOSE_WS_MENU" });
                      dispatch({ type: "SET_SCREEN", screen: "workspaces" });
                    }}
                    style={{
                      textAlign: "left",
                      fontFamily: "inherit",
                      fontSize: 12,
                      fontWeight: 600,
                      color: "var(--txt2)",
                      background: "none",
                      border: "none",
                      cursor: "pointer",
                      padding: "8px 9px",
                      borderRadius: 7,
                    }}
                  >
                    ⛭ Manage workspaces…
                  </button>
                </div>
              </div>
            </>
          )}
        </div>

        {/* Spacer */}
        <div style={{ flex: 1 }} />

        {/* Centered search */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 9,
            width: 480,
            maxWidth: "46vw",
            height: 32,
            background: "var(--panel2)",
            border: "1px solid var(--border)",
            borderRadius: 9,
            padding: "0 11px",
            color: "var(--txt3)",
          }}
        >
          <span style={{ fontSize: 13, flexShrink: 0 }}>⌕</span>
          <input
            ref={searchRef}
            value={state.search}
            onChange={(e) =>
              dispatch({ type: "SET_SEARCH", search: e.target.value })
            }
            placeholder="Search sessions, projects, hosts…"
            style={{
              flex: 1,
              minWidth: 0,
              background: "none",
              border: "none",
              outline: "none",
              color: "var(--txt)",
              fontFamily: "inherit",
              fontSize: 12.5,
            }}
          />
          <span
            className="mono"
            style={{
              fontSize: 10,
              padding: "2px 6px",
              border: "1px solid var(--border2)",
              borderRadius: 5,
              color: "var(--txt3)",
              flexShrink: 0,
            }}
          >
            ⌘K
          </span>
        </div>

        {/* Spacer */}
        <div style={{ flex: 1 }} />

        {/* Settings button */}
        <button
          onClick={onOpenSettings ?? (() => dispatch({ type: "SET_SCREEN", screen: "settings" }))}
          title="Settings"
          style={{
            width: 32,
            height: 32,
            borderRadius: 8,
            border: "1px solid var(--border)",
            background:
              state.screen === "settings" ? "var(--accent-bg)" : "var(--panel2)",
            color:
              state.screen === "settings" ? "var(--accent2)" : "var(--txt2)",
            cursor: "pointer",
            fontSize: 15,
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
            fontSize: 14,
          }}
        >
          {themeIcon}
        </button>

        {/* New session */}
        <button
          onClick={onNewSession ?? (() => dispatch({ type: "SET_SCREEN", screen: "new-session" }))}
          style={{
            display: "flex",
            alignItems: "center",
            gap: 7,
            border: "none",
            background: "var(--accent)",
            color: "#fff",
            fontWeight: 700,
            fontSize: 12.5,
            padding: "8px 14px",
            borderRadius: 9,
            cursor: "pointer",
            fontFamily: "inherit",
          }}
        >
          + New session
        </button>
      </header>

      {/* ===== BODY ===== */}
      <div style={{ display: "flex", flex: 1, overflow: "hidden" }}>
        {/* ---------- SIDEBAR ---------- */}
        <aside
          style={{
            width: 312,
            flexShrink: 0,
            borderRight: "1px solid var(--border)",
            background: "var(--panel)",
            display: "flex",
            flexDirection: "column",
            overflow: "hidden",
          }}
        >
          {/* Overview entry */}
          <div
            onClick={goOverview}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 10,
              margin: "12px 12px 6px",
              padding: "10px 12px",
              borderRadius: 9,
              cursor: "pointer",
              fontWeight: 600,
              background:
                state.screen === "overview" ? "var(--accent-bg)" : "transparent",
              color:
                state.screen === "overview" ? "var(--accent2)" : "var(--txt)",
            }}
            data-testid="overview-entry"
          >
            <span style={{ fontSize: 14 }}>▦</span> Overview
            <span style={{ flex: 1 }} />
            <span style={{ fontSize: 11, color: "var(--txt3)" }}>
              {counts.total}
            </span>
          </div>

          {/* Filter chips */}
          <div
            style={{
              display: "flex",
              gap: 6,
              padding: "6px 12px",
              flexWrap: "wrap",
            }}
          >
            {filters.map((f) => (
              <Chip
                key={f.value}
                label={f.label}
                count={f.value !== "All" ? f.count : undefined}
                active={state.filter === f.value}
                onClick={() =>
                  dispatch({ type: "SET_FILTER", filter: f.value })
                }
              />
            ))}
          </div>

          {/* Session list */}
          <div
            style={{
              flex: 1,
              overflowY: "auto",
              padding: "6px 10px 14px",
            }}
          >
            {sidebarList.map((row) => (
              <SidebarRow
                key={row.id}
                name={row.name}
                state={row.state}
                host_label={row.host_label}
                project={row.project}
                selected={state.selectedId === row.id}
                onSelect={() =>
                  dispatch({ type: "SELECT_SESSION", id: row.id })
                }
              />
            ))}
          </div>

          {/* Host health footer */}
          <div
            style={{
              borderTop: "1px solid var(--border)",
              padding: "11px 14px",
              display: "flex",
              flexDirection: "column",
              gap: 8,
            }}
          >
            <span
              style={{
                fontSize: 9.5,
                fontWeight: 700,
                letterSpacing: 1,
                textTransform: "uppercase",
                color: "var(--txt3)",
              }}
            >
              Hosts
            </span>
            {hosts.map((h) => (
              <div
                key={h.id}
                style={{ display: "flex", alignItems: "center", gap: 8 }}
              >
                <span
                  style={{
                    width: 7,
                    height: 7,
                    borderRadius: "50%",
                    background: hostStatusColor(h.status),
                    animation:
                      h.status === "Reconnecting"
                        ? "dotpulse 1.4s infinite"
                        : undefined,
                  }}
                />
                <span
                  className="mono"
                  style={{ fontSize: 11, color: "var(--txt2)", flex: 1 }}
                >
                  {h.label}
                </span>
                <span
                  style={{
                    fontSize: 10,
                    color: hostStatusColor(h.status),
                  }}
                >
                  {hostStatusLabel(h.status)}
                </span>
              </div>
            ))}
          </div>
        </aside>

        {/* ---------- MAIN ---------- */}
        <main style={{ flex: 1, overflowY: "auto", position: "relative" }}>
          {children}
        </main>
      </div>
    </div>
  );
}
