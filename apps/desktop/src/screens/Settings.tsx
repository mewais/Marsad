/**
 * Settings screen — Notifications / Hosts / Session defaults / General
 *
 * Tab nav is local component state (no store field for active settings tab).
 * Theme and glass are wired to ThemeProvider via useTheme().
 * Host add/remove: add-remote form uses local state + dispatch SET_SCREEN
 * (no dedicated backend command yet — NOTE: backend pending for addHost/removeHost).
 * Notification preferences: all local state, NOTE: backend command pending.
 * Background picker: local state only, NOTE: backend command pending.
 * Behavior toggles (restore-on-launch, confirm-before-stop): local state,
 * NOTE: backend command pending.
 * Session defaults (model + posture): local state,
 * NOTE: backend command pending for persisting session defaults.
 */

import React, { useState } from "react";
import { useVm } from "../bridge/useVm";
import { useTheme } from "../theme/ThemeProvider";
import { Toggle } from "../components/Toggle";
import { Button } from "../components/Button";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type SettingsTab = "Notifications" | "Hosts" | "Session defaults" | "General";

// Notification events tracked in the design
type NotifEvent =
  | "gate_needs_you"
  | "gate_error"
  | "session_done"
  | "session_started"
  | "host_unreachable";

interface NotifPref {
  enabled: boolean;
  priority: "urgent" | "normal" | "low";
  sound: boolean;
  group: boolean;
  dnd: boolean;
}

const NOTIF_EVENT_LABELS: Record<NotifEvent, string> = {
  gate_needs_you: "Gate needs you",
  gate_error: "Gate errored",
  session_done: "Session done",
  session_started: "Session started",
  host_unreachable: "Host unreachable",
};

const DEFAULT_NOTIF_PREFS: Record<NotifEvent, NotifPref> = {
  gate_needs_you: { enabled: true, priority: "urgent", sound: true, group: false, dnd: false },
  gate_error: { enabled: true, priority: "urgent", sound: true, group: false, dnd: false },
  session_done: { enabled: true, priority: "normal", sound: false, group: true, dnd: true },
  session_started: { enabled: false, priority: "low", sound: false, group: true, dnd: true },
  host_unreachable: { enabled: true, priority: "urgent", sound: true, group: false, dnd: false },
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const S: Record<string, React.CSSProperties> = {
  root: {
    display: "flex",
    flexDirection: "column",
    height: "100%",
    overflow: "hidden",
    color: "var(--txt)",
  },
  header: {
    padding: "22px 26px 0",
    borderBottom: "1px solid var(--border)",
    flexShrink: 0,
  },
  title: {
    fontSize: 21,
    fontWeight: 700,
    margin: "0 0 14px",
    letterSpacing: -0.3,
    color: "var(--txt)",
  },
  tabBar: {
    display: "flex",
    gap: 2,
    marginBottom: -1,
  },
  body: {
    flex: 1,
    overflowY: "auto",
    padding: "24px 26px",
  },
  section: {
    marginBottom: 28,
  },
  sectionTitle: {
    fontSize: 11,
    fontWeight: 700,
    letterSpacing: 0.8,
    textTransform: "uppercase" as const,
    color: "var(--txt3)",
    marginBottom: 10,
  },
  row: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    padding: "10px 0",
    borderBottom: "1px solid var(--border)",
  },
  rowLabel: {
    fontSize: 13,
    fontWeight: 500,
    color: "var(--txt)",
  },
  rowSub: {
    fontSize: 11,
    color: "var(--txt3)",
    marginTop: 2,
  },
};

function TabButton({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      data-testid={`settings-tab-${label.toLowerCase().replace(/\s+/g, "-")}`}
      onClick={onClick}
      style={{
        background: "transparent",
        border: "none",
        borderBottom: active ? "2px solid var(--accent)" : "2px solid transparent",
        color: active ? "var(--txt)" : "var(--txt3)",
        fontWeight: active ? 700 : 500,
        fontSize: 13,
        padding: "8px 14px",
        cursor: "pointer",
        fontFamily: "inherit",
        letterSpacing: -0.1,
        transition: "color 0.15s",
      }}
    >
      {label}
    </button>
  );
}

// ---------------------------------------------------------------------------
// Notifications tab
// ---------------------------------------------------------------------------

const NOTIF_EVENTS: NotifEvent[] = [
  "gate_needs_you",
  "gate_error",
  "session_done",
  "session_started",
  "host_unreachable",
];

const PRIORITY_OPTS: Array<"urgent" | "normal" | "low"> = ["urgent", "normal", "low"];

function NotificationsTab() {
  // NOTE: backend command pending — notif prefs are local state only
  const [prefs, setPrefs] = useState<Record<NotifEvent, NotifPref>>(
    DEFAULT_NOTIF_PREFS
  );

  function setPref<K extends keyof NotifPref>(
    ev: NotifEvent,
    key: K,
    val: NotifPref[K]
  ) {
    setPrefs((p) => ({ ...p, [ev]: { ...p[ev], [key]: val } }));
  }

  return (
    <div>
      {/* Per-event toggles */}
      <div style={S.section}>
        <div style={S.sectionTitle}>Per-event settings</div>
        <div
          style={{
            border: "1px solid var(--border)",
            borderRadius: 10,
            overflow: "hidden",
          }}
        >
          {/* Header row */}
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "1fr 110px 56px 56px 56px",
              gap: 8,
              padding: "8px 14px",
              background: "var(--panel2)",
              fontSize: 10,
              fontWeight: 700,
              letterSpacing: 0.5,
              textTransform: "uppercase",
              color: "var(--txt3)",
            }}
          >
            <span>Event</span>
            <span style={{ textAlign: "center" }}>Priority</span>
            <span style={{ textAlign: "center" }}>Sound</span>
            <span style={{ textAlign: "center" }}>Group</span>
            <span style={{ textAlign: "center" }}>DND</span>
          </div>
          {NOTIF_EVENTS.map((ev, i) => {
            const p = prefs[ev];
            return (
              <div
                key={ev}
                data-testid={`notif-row-${ev}`}
                style={{
                  display: "grid",
                  gridTemplateColumns: "1fr 110px 56px 56px 56px",
                  gap: 8,
                  alignItems: "center",
                  padding: "10px 14px",
                  borderTop: i === 0 ? "none" : "1px solid var(--border)",
                  background: p.enabled ? "transparent" : "var(--panel2)",
                  opacity: p.enabled ? 1 : 0.55,
                }}
              >
                {/* Event label + enabled toggle */}
                <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                  <Toggle
                    checked={p.enabled}
                    onChange={(v) => setPref(ev, "enabled", v)}
                  />
                  <span style={{ fontSize: 13, fontWeight: 500, color: "var(--txt)" }}>
                    {NOTIF_EVENT_LABELS[ev]}
                  </span>
                </div>
                {/* Priority picker */}
                <select
                  value={p.priority}
                  disabled={!p.enabled}
                  onChange={(e) =>
                    setPref(ev, "priority", e.target.value as NotifPref["priority"])
                  }
                  style={{
                    background: "var(--panel2)",
                    color: "var(--txt)",
                    border: "1px solid var(--border2)",
                    borderRadius: 6,
                    fontSize: 11,
                    padding: "3px 6px",
                    cursor: p.enabled ? "pointer" : "default",
                    fontFamily: "inherit",
                  }}
                >
                  {PRIORITY_OPTS.map((o) => (
                    <option key={o} value={o}>
                      {o.charAt(0).toUpperCase() + o.slice(1)}
                    </option>
                  ))}
                </select>
                {/* Sound */}
                <div style={{ display: "flex", justifyContent: "center" }}>
                  <Toggle
                    checked={p.sound}
                    onChange={(v) => setPref(ev, "sound", v)}
                    disabled={!p.enabled}
                  />
                </div>
                {/* Group */}
                <div style={{ display: "flex", justifyContent: "center" }}>
                  <Toggle
                    checked={p.group}
                    onChange={(v) => setPref(ev, "group", v)}
                    disabled={!p.enabled}
                  />
                </div>
                {/* DND */}
                <div style={{ display: "flex", justifyContent: "center" }}>
                  <Toggle
                    checked={p.dnd}
                    onChange={(v) => setPref(ev, "dnd", v)}
                    disabled={!p.enabled}
                  />
                </div>
              </div>
            );
          })}
        </div>
      </div>

      {/* OS pop-up preview panel (static) */}
      <div style={S.section}>
        <div style={S.sectionTitle}>Approximate OS notification preview</div>
        <div
          style={{
            border: "1px solid var(--border2)",
            borderRadius: 12,
            background: "var(--elev)",
            padding: "14px 16px",
            maxWidth: 360,
            boxShadow: "0 4px 20px rgba(0,0,0,0.25)",
          }}
        >
          <div style={{ display: "flex", alignItems: "flex-start", gap: 10 }}>
            <div
              style={{
                width: 32,
                height: 32,
                borderRadius: 8,
                background: "var(--accent)",
                flexShrink: 0,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                fontSize: 16,
              }}
            >
              M
            </div>
            <div>
              <div style={{ fontSize: 12, fontWeight: 700, color: "var(--txt)", marginBottom: 2 }}>
                Marsad · Gate needs you
              </div>
              <div style={{ fontSize: 11, color: "var(--txt2)" }}>
                api-auth-refactor is waiting for your approval on a Permission gate.
              </div>
              <div style={{ fontSize: 10, color: "var(--txt3)", marginTop: 4 }}>
                now
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Hosts tab
// ---------------------------------------------------------------------------

function HostStatusDot({ status }: { status: string }) {
  const color =
    status === "Connected"
      ? "var(--done)"
      : status === "Reconnecting"
      ? "var(--needs)"
      : "var(--error)";
  return (
    <span
      style={{
        width: 8,
        height: 8,
        borderRadius: "50%",
        background: color,
        display: "inline-block",
        flexShrink: 0,
      }}
    />
  );
}

function HostsTab() {
  const { hosts } = useVm();
  // NOTE: backend command pending for addHost / removeHost
  const [label, setLabel] = useState("");
  const [target, setTarget] = useState("");
  const [added, setAdded] = useState<boolean>(false);

  function handleAdd() {
    if (!label.trim() || !target.trim()) return;
    // NOTE: no addHost api command exists yet — local ack only
    setAdded(true);
    setLabel("");
    setTarget("");
    setTimeout(() => setAdded(false), 2500);
  }

  return (
    <div>
      <div style={S.section}>
        <div style={S.sectionTitle}>Connected hosts</div>
        <div
          style={{
            border: "1px solid var(--border)",
            borderRadius: 10,
            overflow: "hidden",
          }}
        >
          {hosts.map((h, i) => (
            <div
              key={h.id}
              data-testid={`host-row-${h.id}`}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 12,
                padding: "12px 16px",
                borderTop: i === 0 ? "none" : "1px solid var(--border)",
                background: "transparent",
              }}
            >
              <HostStatusDot status={h.status} />
              <div style={{ flex: 1, minWidth: 0 }}>
                <div
                  style={{
                    fontSize: 13,
                    fontWeight: 600,
                    color: "var(--txt)",
                    display: "flex",
                    alignItems: "center",
                    gap: 6,
                  }}
                >
                  {h.label}
                  {h.is_local && (
                    <span
                      style={{
                        fontSize: 9,
                        fontWeight: 700,
                        letterSpacing: 0.3,
                        padding: "1px 5px",
                        borderRadius: 4,
                        color: "var(--txt3)",
                        background: "var(--panel2)",
                        textTransform: "uppercase",
                      }}
                    >
                      this machine
                    </span>
                  )}
                </div>
                <div style={{ fontSize: 11, color: "var(--txt3)", marginTop: 2 }}>
                  {h.target} · {h.session_count} session{h.session_count !== 1 ? "s" : ""}
                </div>
              </div>
              <div style={{ fontSize: 11, color: "var(--txt3)", minWidth: 80, textAlign: "right" }}>
                {h.status}
              </div>
              {h.removable ? (
                <Button
                  variant="danger"
                  size="sm"
                  // NOTE: removeHost backend command pending
                  onClick={() => {}}
                  style={{ opacity: 0.65 }}
                >
                  Remove
                </Button>
              ) : (
                <Button variant="secondary" size="sm" disabled>
                  Protected
                </Button>
              )}
            </div>
          ))}
        </div>
      </div>

      {/* Add remote form */}
      <div style={S.section}>
        <div style={S.sectionTitle}>Add remote host</div>
        <div
          style={{
            border: "1px solid var(--border)",
            borderRadius: 10,
            padding: "16px",
            display: "flex",
            flexDirection: "column",
            gap: 10,
          }}
        >
          <div style={{ display: "flex", gap: 10 }}>
            <div style={{ flex: 1 }}>
              <label style={{ fontSize: 11, color: "var(--txt3)", display: "block", marginBottom: 4 }}>
                Label
              </label>
              <input
                data-testid="host-add-label"
                value={label}
                onChange={(e) => setLabel(e.target.value)}
                placeholder="e.g. hetzner-cx21"
                style={{
                  width: "100%",
                  background: "var(--panel2)",
                  border: "1px solid var(--border2)",
                  borderRadius: 7,
                  color: "var(--txt)",
                  fontSize: 13,
                  padding: "8px 10px",
                  fontFamily: "inherit",
                  boxSizing: "border-box",
                }}
              />
            </div>
            <div style={{ flex: 2 }}>
              <label style={{ fontSize: 11, color: "var(--txt3)", display: "block", marginBottom: 4 }}>
                SSH target
              </label>
              <input
                data-testid="host-add-target"
                value={target}
                onChange={(e) => setTarget(e.target.value)}
                placeholder="user@hostname"
                style={{
                  width: "100%",
                  background: "var(--panel2)",
                  border: "1px solid var(--border2)",
                  borderRadius: 7,
                  color: "var(--txt)",
                  fontSize: 13,
                  padding: "8px 10px",
                  fontFamily: "inherit",
                  boxSizing: "border-box",
                }}
              />
            </div>
          </div>
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <Button
              data-testid="host-add-btn"
              variant="primary"
              size="sm"
              disabled={!label.trim() || !target.trim()}
              onClick={handleAdd}
            >
              Add remote
            </Button>
            {added && (
              <span style={{ fontSize: 11, color: "var(--done)" }}>
                Queued (backend command pending)
              </span>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Session defaults tab
// ---------------------------------------------------------------------------

const MODELS = [
  "claude-haiku-4-5",
  "claude-sonnet-4-5",
  "claude-opus-4-5",
];

const POSTURES: Array<{ key: "ask" | "auto" | "full"; label: string; desc: string }> = [
  { key: "ask", label: "Ask", desc: "Confirm before every action" },
  { key: "auto", label: "Auto", desc: "Confirm risky actions only" },
  { key: "full", label: "Full auto", desc: "Run without confirmation" },
];

function SessionDefaultsTab() {
  // NOTE: backend command pending for persisting session defaults
  const [model, setModel] = useState("claude-sonnet-4-5");
  const [posture, setPosture] = useState<"ask" | "auto" | "full">("auto");

  return (
    <div>
      <div style={S.section}>
        <div style={S.sectionTitle}>Default model</div>
        <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          {MODELS.map((m) => (
            <button
              key={m}
              data-testid={`defaults-model-${m}`}
              onClick={() => setModel(m)}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 10,
                padding: "10px 14px",
                borderRadius: 9,
                border:
                  model === m
                    ? "1px solid var(--accent)"
                    : "1px solid var(--border2)",
                background: model === m ? "var(--accent-bg)" : "var(--panel2)",
                cursor: "pointer",
                fontFamily: "inherit",
                textAlign: "left",
              }}
            >
              <span
                style={{
                  width: 14,
                  height: 14,
                  borderRadius: "50%",
                  border:
                    model === m
                      ? "4px solid var(--accent)"
                      : "2px solid var(--border2)",
                  background: "transparent",
                  flexShrink: 0,
                  transition: "border 0.15s",
                }}
              />
              <span style={{ fontSize: 13, fontWeight: 500, color: "var(--txt)" }}>
                {m}
              </span>
            </button>
          ))}
        </div>
      </div>

      <div style={S.section}>
        <div style={S.sectionTitle}>Default posture</div>
        <div style={{ display: "flex", gap: 10 }}>
          {POSTURES.map((p) => (
            <button
              key={p.key}
              data-testid={`defaults-posture-${p.key}`}
              onClick={() => setPosture(p.key)}
              style={{
                flex: 1,
                padding: "12px 10px",
                borderRadius: 9,
                border:
                  posture === p.key
                    ? "1px solid var(--accent)"
                    : "1px solid var(--border2)",
                background: posture === p.key ? "var(--accent-bg)" : "var(--panel2)",
                cursor: "pointer",
                fontFamily: "inherit",
                textAlign: "center",
              }}
            >
              <div
                style={{
                  fontSize: 13,
                  fontWeight: 700,
                  color:
                    posture === p.key ? "var(--accent2)" : "var(--txt)",
                  marginBottom: 4,
                }}
              >
                {p.label}
              </div>
              <div style={{ fontSize: 11, color: "var(--txt3)" }}>{p.desc}</div>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// General tab
// ---------------------------------------------------------------------------

type Background = "Gradient" | "Custom image" | "Desktop";

const BG_OPTIONS: Background[] = ["Gradient", "Custom image", "Desktop"];

function GeneralTab() {
  const { theme, glass, toggleTheme, toggleGlass } = useTheme();
  // NOTE: background picker local state — backend command pending
  const [background, setBackground] = useState<Background>("Gradient");
  // NOTE: behavior toggles local state — backend command pending
  const [restoreOnLaunch, setRestoreOnLaunch] = useState(true);
  const [confirmBeforeStop, setConfirmBeforeStop] = useState(true);

  return (
    <div>
      <div style={S.section}>
        <div style={S.sectionTitle}>Appearance</div>
        <div
          style={{
            border: "1px solid var(--border)",
            borderRadius: 10,
            overflow: "hidden",
          }}
        >
          {/* Theme */}
          <div style={S.row}>
            <div style={{ padding: "0 0 0 16px" }}>
              <div style={S.rowLabel}>Theme</div>
              <div style={S.rowSub}>Interface color scheme</div>
            </div>
            <div
              style={{
                display: "flex",
                gap: 6,
                padding: "0 16px",
              }}
            >
              {(["dark", "light"] as const).map((t) => (
                <button
                  key={t}
                  data-testid={`general-theme-${t}`}
                  onClick={() => toggleTheme()}
                  style={{
                    padding: "5px 12px",
                    borderRadius: 7,
                    border:
                      theme === t
                        ? "1px solid var(--accent)"
                        : "1px solid var(--border2)",
                    background: theme === t ? "var(--accent-bg)" : "var(--panel2)",
                    color: theme === t ? "var(--accent2)" : "var(--txt2)",
                    fontSize: 12,
                    fontWeight: 600,
                    cursor: "pointer",
                    fontFamily: "inherit",
                    textTransform: "capitalize",
                  }}
                >
                  {t}
                </button>
              ))}
            </div>
          </div>

          {/* Glass */}
          <div style={{ ...S.row, borderTop: "1px solid var(--border)" }}>
            <div style={{ padding: "0 0 0 16px" }}>
              <div style={S.rowLabel}>Translucent (glass)</div>
              <div style={S.rowSub}>Blur + backdrop transparency</div>
            </div>
            <div style={{ padding: "0 16px" }} data-testid="general-glass-toggle-wrap">
              <Toggle
                checked={glass}
                onChange={toggleGlass}
              />
            </div>
          </div>

          {/* Background */}
          <div style={{ ...S.row, borderTop: "1px solid var(--border)", alignItems: "flex-start" }}>
            <div style={{ padding: "10px 0 10px 16px" }}>
              <div style={S.rowLabel}>Background</div>
              <div style={S.rowSub}>Wallpaper shown in glass mode</div>
            </div>
            <div
              style={{
                display: "flex",
                gap: 6,
                padding: "10px 16px",
              }}
            >
              {BG_OPTIONS.map((bg) => (
                <button
                  key={bg}
                  data-testid={`general-bg-${bg.toLowerCase().replace(/\s+/g, "-")}`}
                  onClick={() => setBackground(bg)}
                  style={{
                    padding: "5px 10px",
                    borderRadius: 7,
                    border:
                      background === bg
                        ? "1px solid var(--accent)"
                        : "1px solid var(--border2)",
                    background: background === bg ? "var(--accent-bg)" : "var(--panel2)",
                    color: background === bg ? "var(--accent2)" : "var(--txt2)",
                    fontSize: 11,
                    fontWeight: 600,
                    cursor: "pointer",
                    fontFamily: "inherit",
                  }}
                >
                  {bg}
                </button>
              ))}
            </div>
          </div>
        </div>
      </div>

      <div style={S.section}>
        <div style={S.sectionTitle}>Behavior</div>
        <div
          style={{
            border: "1px solid var(--border)",
            borderRadius: 10,
            overflow: "hidden",
          }}
        >
          <div style={S.row}>
            <div style={{ padding: "0 0 0 16px" }}>
              <div style={S.rowLabel}>Restore workspace on launch</div>
              <div style={S.rowSub}>Re-open the active workspace when Marsad starts</div>
            </div>
            <div style={{ padding: "0 16px" }} data-testid="general-restore-toggle-wrap">
              <Toggle
                checked={restoreOnLaunch}
                onChange={setRestoreOnLaunch}
              />
            </div>
          </div>
          <div style={{ ...S.row, borderTop: "1px solid var(--border)" }}>
            <div style={{ padding: "0 0 0 16px" }}>
              <div style={S.rowLabel}>Confirm before stopping a session</div>
              <div style={S.rowSub}>Show a confirmation dialog before sending stop</div>
            </div>
            <div style={{ padding: "0 16px" }} data-testid="general-confirm-toggle-wrap">
              <Toggle
                checked={confirmBeforeStop}
                onChange={setConfirmBeforeStop}
              />
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Settings root
// ---------------------------------------------------------------------------

const TABS: SettingsTab[] = ["Notifications", "Hosts", "Session defaults", "General"];

export function Settings() {
  const [activeTab, setActiveTab] = useState<SettingsTab>("Notifications");

  return (
    <div style={S.root} data-testid="settings-screen">
      <div style={S.header}>
        <h1 style={S.title}>Settings</h1>
        <div style={S.tabBar}>
          {TABS.map((t) => (
            <TabButton
              key={t}
              label={t}
              active={activeTab === t}
              onClick={() => setActiveTab(t)}
            />
          ))}
        </div>
      </div>

      <div style={S.body}>
        {activeTab === "Notifications" && <NotificationsTab />}
        {activeTab === "Hosts" && <HostsTab />}
        {activeTab === "Session defaults" && <SessionDefaultsTab />}
        {activeTab === "General" && <GeneralTab />}
      </div>
    </div>
  );
}
