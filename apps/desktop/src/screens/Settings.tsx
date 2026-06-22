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

import React, { useState, useEffect } from "react";
import { useVm } from "../bridge/useVm";
import { useTheme } from "../theme/ThemeProvider";
import { Toggle } from "../components/Toggle";
import { MODELS as MODEL_LIST, DEFAULT_MODEL } from "../bridge/models";
import { getAutostart, setAutostart } from "../bridge/api";

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

const NOTIF_EVENT_DESCS: Record<NotifEvent, string> = {
  gate_needs_you: "A permission gate, plan, or question is waiting",
  gate_error: "A session hits an error and stops",
  session_done: "A session completes its work and goes idle",
  session_started: "A new session boots and begins working",
  host_unreachable: "A host stops responding",
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
  titleRow: {
    display: "flex",
    alignItems: "baseline",
    justifyContent: "flex-start",
    gap: 12,
    marginBottom: 4,
  },
  title: {
    fontSize: 26,
    fontWeight: 700,
    margin: 0,
    letterSpacing: -0.3,
    color: "var(--txt)",
  },
  titleTab: {
    fontSize: 15,
    color: "var(--txt2)",
    fontWeight: 400,
  },
  subtitle: {
    fontSize: 15,
    color: "var(--txt3)",
    margin: "0 0 22px",
    textAlign: "left" as const,
  },
  tabBar: {
    display: "flex",
    gap: 4,
    marginBottom: -1,
    justifyContent: "flex-start",
  },
  body: {
    flex: 1,
    overflowY: "auto",
    padding: "24px 26px 48px",
  },
  // Content sits in a centered column (like the session window's tab content);
  // only the header/tabs are full-width left-aligned.
  bodyInner: {
    maxWidth: 1100,
    margin: "0 auto",
  },
  sectionLabel: {
    fontSize: 13.5,
    fontWeight: 700,
    letterSpacing: 0.5,
    textTransform: "uppercase" as const,
    color: "var(--txt3)",
    marginBottom: 10,
  },
  sectionLabelBold: {
    fontSize: 14.5,
    fontWeight: 700,
    color: "var(--txt2)",
    marginBottom: 9,
  },
};

const TAB_SUBTITLES: Record<SettingsTab, string> = {
  Notifications: "Choose what pops up on your desktop when Marsad is minimized or in the background.",
  Hosts: "Manage the local machine and the remote Linux hosts where sessions run.",
  "Session defaults": "Set the defaults applied to every new session.",
  General: "Appearance and app behavior.",
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
        fontWeight: active ? 600 : 500,
        fontSize: 15,
        padding: "9px 13px",
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

const PRIORITY_COLORS: Record<"urgent" | "normal" | "low", { color: string; bg: string }> = {
  urgent: { color: "var(--error)", bg: "rgba(224,89,74,0.12)" },
  normal: { color: "var(--needs)", bg: "rgba(241,168,58,0.12)" },
  low: { color: "var(--txt3)", bg: "var(--panel2)" },
};

// Popup preview specs (static illustrative data)
const POPUP_SPECS = [
  {
    host: "localhost",
    priority: "urgent" as const,
    color: "var(--error)",
    icon: "⚠",
    title: "api-gateway needs you",
    body: "api-auth-refactor is waiting for your approval on a Permission gate.",
    actions: [
      { l: "Approve", bg: "var(--done)", fg: "#0e1116", bd: "none" },
      { l: "Reject", bg: "var(--panel2)", fg: "var(--txt2)", bd: "1px solid var(--border2)" },
      { l: "Open", bg: "var(--panel2)", fg: "var(--txt2)", bd: "1px solid var(--border2)" },
    ],
    dim: 1,
  },
  {
    host: "hetzner-fra1",
    priority: "urgent" as const,
    color: "var(--error)",
    icon: "✕",
    title: "payments-svc errored",
    body: "TypeError in compute/fee (checkout.py:88)",
    actions: [
      { l: "Open", bg: "var(--done)", fg: "#0e1116", bd: "none" },
      { l: "Retry", bg: "var(--panel2)", fg: "var(--txt2)", bd: "1px solid var(--border2)" },
    ],
    dim: 0.8,
  },
  {
    host: "gpu-box",
    priority: "low" as const,
    color: "var(--done)",
    icon: "✓",
    title: "etl-pipeline finished",
    body: "9 files changed · batch job 6.7s faster",
    actions: [
      { l: "Open", bg: "var(--done)", fg: "#0e1116", bd: "none" },
      { l: "Dismiss", bg: "var(--panel2)", fg: "var(--txt2)", bd: "1px solid var(--border2)" },
    ],
    dim: 0.65,
  },
];

function NotificationsTab() {
  // NOTE: backend command pending — notif prefs are local state only
  const [prefs, setPrefs] = useState<Record<NotifEvent, NotifPref>>(
    DEFAULT_NOTIF_PREFS
  );
  // Delivery globals (sound, group, dnd) — local state
  const [playSound, setPlaySound] = useState(true);
  const [groupAlerts, setGroupAlerts] = useState(true);
  const [doNotDisturb, setDoNotDisturb] = useState(false);

  function setPref<K extends keyof NotifPref>(
    ev: NotifEvent,
    key: K,
    val: NotifPref[K]
  ) {
    setPrefs((p) => ({ ...p, [ev]: { ...p[ev], [key]: val } }));
  }

  return (
    <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 26, alignItems: "start" }}>
      {/* LEFT: Notify me when + Delivery */}
      <div style={{ display: "flex", flexDirection: "column", gap: 9 }}>
        <div style={{ ...S.sectionLabel, marginBottom: 2 }}>Notify me when…</div>

        {NOTIF_EVENTS.map((ev) => {
          const p = prefs[ev];
          const priStyle = PRIORITY_COLORS[p.priority];
          return (
            <div
              key={ev}
              data-testid={`notif-row-${ev}`}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 13,
                padding: "13px 15px",
                borderRadius: 11,
                background: "var(--panel)",
                border: "1px solid var(--border)",
              }}
            >
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontWeight: 600, fontSize: 15.5, color: "var(--txt)" }}>
                  {NOTIF_EVENT_LABELS[ev]}
                </div>
                <div style={{ fontSize: 14, color: "var(--txt3)", marginTop: 1 }}>
                  {NOTIF_EVENT_DESCS[ev]}
                </div>
              </div>
              <span
                style={{
                  fontSize: 11.5,
                  fontWeight: 700,
                  letterSpacing: 0.3,
                  textTransform: "uppercase",
                  padding: "2px 7px",
                  borderRadius: 5,
                  color: priStyle.color,
                  background: priStyle.bg,
                  flexShrink: 0,
                  cursor: p.enabled ? "pointer" : "default",
                }}
                onClick={() => {
                  if (!p.enabled) return;
                  const opts = PRIORITY_OPTS;
                  const idx = opts.indexOf(p.priority);
                  setPref(ev, "priority", opts[(idx + 1) % opts.length]);
                }}
                title="Click to cycle priority"
              >
                {p.priority}
              </span>
              <div style={{ flexShrink: 0 }}>
                <Toggle
                  checked={p.enabled}
                  onChange={(v) => setPref(ev, "enabled", v)}
                />
              </div>
            </div>
          );
        })}

        {/* Delivery section */}
        <div style={{ ...S.sectionLabel, marginTop: 14, marginBottom: 2 }}>Delivery</div>

        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 13,
            padding: "13px 15px",
            borderRadius: 11,
            background: "var(--panel)",
            border: "1px solid var(--border)",
          }}
        >
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontWeight: 600, fontSize: 15.5, color: "var(--txt)" }}>Play a sound</div>
            <div style={{ fontSize: 14, color: "var(--txt3)", marginTop: 1 }}>
              Audible alerts only.
            </div>
          </div>
          <Toggle checked={playSound} onChange={setPlaySound} />
        </div>

        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 13,
            padding: "13px 15px",
            borderRadius: 11,
            background: "var(--panel)",
            border: "1px solid var(--border)",
          }}
        >
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontWeight: 600, fontSize: 15.5, color: "var(--txt)" }}>Group similar alerts</div>
            <div style={{ fontSize: 14, color: "var(--txt3)", marginTop: 1 }}>
              Collapses repeated "session done" notifications.
            </div>
          </div>
          <Toggle checked={groupAlerts} onChange={setGroupAlerts} />
        </div>

        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 13,
            padding: "13px 15px",
            borderRadius: 11,
            background: "var(--panel)",
            border: "1px solid var(--border)",
          }}
        >
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontWeight: 600, fontSize: 15.5, color: "var(--txt)" }}>Do Not Disturb</div>
            <div style={{ fontSize: 14, color: "var(--txt3)", marginTop: 1 }}>
              DND-off respects the dashboard-dnd-bypass tile.
            </div>
          </div>
          <Toggle checked={doNotDisturb} onChange={setDoNotDisturb} />
        </div>

        <div
          className="mono"
          style={{
            fontSize: 13,
            color: "var(--txt3)",
            marginTop: 8,
            lineHeight: 1.5,
          }}
        >
          Delivered to this workstation via your desktop's notification service.{" "}
          No phone push, no cloud — per the local-first design.
        </div>
      </div>

      {/* RIGHT: Pop-up preview */}
      <div>
        <div style={{ ...S.sectionLabel, marginBottom: 7 }}>Pop-up preview</div>
        <div
          style={{
            display: "flex",
            alignItems: "flex-start",
            gap: 7,
            fontSize: 13.5,
            color: "var(--txt3)",
            marginBottom: 13,
            lineHeight: 1.45,
          }}
        >
          <span style={{ flexShrink: 0 }}>ⓘ</span>
          <span>
            Approximate. Your desktop environment (GNOME, KDE, …) renders the final style —
            Marsad controls the content and actions, not the chrome.
          </span>
        </div>

        <div
          style={{
            borderRadius: 16,
            padding: "18px 16px",
            border: "1px dashed var(--border2)",
            background:
              "linear-gradient(160deg, color-mix(in srgb, var(--accent) 14%, var(--bg)), var(--bg) 70%)",
          }}
        >
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 7,
              marginBottom: 14,
              opacity: 0.7,
            }}
          >
            <span
              style={{
                width: 6,
                height: 6,
                borderRadius: "50%",
                background: "var(--txt3)",
              }}
            />
            <span
              className="mono"
              style={{ fontSize: 12, color: "var(--txt3)" }}
            >
              your desktop · top-right corner
            </span>
          </div>

          {POPUP_SPECS.map((p, i) => {
            const priStyle = PRIORITY_COLORS[p.priority];
            return (
              <div
                key={i}
                style={{
                  background: "var(--elev)",
                  border: `1px solid var(--border2)`,
                  borderLeft: `3px solid ${p.color}`,
                  borderRadius: 13,
                  boxShadow: "0 4px 16px rgba(0,0,0,0.3)",
                  marginBottom: 12,
                  overflow: "hidden",
                  opacity: p.dim,
                }}
              >
                <div
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 8,
                    padding: "8px 13px",
                    borderBottom: "1px solid var(--border)",
                  }}
                >
                  <div
                    style={{
                      width: 17,
                      height: 17,
                      borderRadius: 5,
                      background: "linear-gradient(135deg, var(--accent), var(--accent2))",
                      display: "flex",
                      alignItems: "center",
                      justifyContent: "center",
                      color: "#fff",
                      fontWeight: 800,
                      fontSize: 12,
                    }}
                  >
                    M
                  </div>
                  <span style={{ fontSize: 13, fontWeight: 700, color: "var(--txt2)" }}>
                    Marsad
                  </span>
                  <span className="mono" style={{ fontSize: 11.5, color: "var(--txt3)" }}>
                    {p.host}
                  </span>
                  <span style={{ flex: 1 }} />
                  <span
                    style={{
                      fontSize: 10.5,
                      fontWeight: 700,
                      letterSpacing: 0.3,
                      textTransform: "uppercase",
                      padding: "1px 6px",
                      borderRadius: 5,
                      color: priStyle.color,
                      background: priStyle.bg,
                    }}
                  >
                    {p.priority}
                  </span>
                  <span className="mono" style={{ fontSize: 11.5, color: "var(--txt3)" }}>
                    now
                  </span>
                </div>
                <div style={{ padding: "12px 13px" }}>
                  <div
                    style={{
                      display: "flex",
                      alignItems: "flex-start",
                      gap: 7,
                      marginBottom: 5,
                    }}
                  >
                    <span
                      style={{
                        fontSize: 13.5,
                        color: p.color,
                        lineHeight: 1.5,
                        flexShrink: 0,
                      }}
                    >
                      {p.icon}
                    </span>
                    <span style={{ fontWeight: 700, fontSize: 15, lineHeight: 1.35 }}>
                      {p.title}
                    </span>
                  </div>
                  <div style={{ fontSize: 14, color: "var(--txt2)", lineHeight: 1.45 }}>
                    {p.body}
                  </div>
                  <div style={{ display: "flex", gap: 7, marginTop: 11 }}>
                    {p.actions.map((a, ai) => (
                      <span
                        key={ai}
                        style={{
                          fontFamily: "inherit",
                          fontWeight: 700,
                          fontSize: 13.5,
                          padding: "6px 12px",
                          borderRadius: 7,
                          background: a.bg,
                          color: a.fg,
                          border: a.bd,
                        }}
                      >
                        {a.l}
                      </span>
                    ))}
                  </div>
                </div>
              </div>
            );
          })}
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
        width: 9,
        height: 9,
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
    <div style={{ maxWidth: 860 }}>
      {/* Registered hosts */}
      <div style={{ ...S.sectionLabel, marginBottom: 10 }}>Registered hosts</div>
      <div style={{ display: "flex", flexDirection: "column", gap: 9, marginBottom: 26 }}>
        {hosts.map((h) => (
          <div
            key={h.id}
            data-testid={`host-row-${h.id}`}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 13,
              padding: "13px 15px",
              borderRadius: 11,
              background: "var(--panel)",
              border: "1px solid var(--border)",
            }}
          >
            <HostStatusDot status={h.status} />
            <div style={{ flex: 1, minWidth: 0 }}>
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <span style={{ fontWeight: 600, fontSize: 15.5, color: "var(--txt)" }}>
                  {h.label}
                </span>
                {h.is_local && (
                  <span
                    style={{
                      fontSize: 11.5,
                      fontWeight: 700,
                      padding: "1px 6px",
                      borderRadius: 5,
                      background: "var(--panel2)",
                      color: "var(--txt3)",
                      textTransform: "uppercase",
                    }}
                  >
                    this machine
                  </span>
                )}
              </div>
              <div
                className="mono"
                style={{ fontSize: 13, color: "var(--txt3)", marginTop: 2 }}
              >
                {h.target} · {h.session_count} session{h.session_count !== 1 ? "s" : ""}
              </div>
            </div>
            <span
              style={{
                fontSize: 13.5,
                color:
                  h.status === "Connected"
                    ? "var(--done)"
                    : h.status === "Reconnecting"
                    ? "var(--needs)"
                    : "var(--txt3)",
                fontWeight: 600,
              }}
            >
              {h.status}
            </span>
            {h.removable ? (
              <button
                // NOTE: removeHost backend command pending
                onClick={() => {}}
                style={{
                  fontFamily: "inherit",
                  fontSize: 13.5,
                  fontWeight: 600,
                  padding: "6px 11px",
                  borderRadius: 8,
                  border: "1px solid var(--border2)",
                  background: "transparent",
                  color: "var(--txt2)",
                  cursor: "pointer",
                }}
              >
                Remove
              </button>
            ) : (
              <span
                style={{
                  fontSize: 12.5,
                  fontWeight: 600,
                  padding: "3px 8px",
                  borderRadius: 6,
                  background: "var(--panel2)",
                  color: "var(--txt3)",
                  border: "1px solid var(--border)",
                }}
              >
                Protected
              </span>
            )}
          </div>
        ))}
      </div>

      {/* Add a remote host */}
      <div style={{ ...S.sectionLabel, marginBottom: 10 }}>Add a host</div>
      <div
        style={{
          padding: 15,
          borderRadius: 12,
          border: "1px dashed var(--border2)",
          background: "var(--panel2)",
          display: "flex",
          flexDirection: "column",
          gap: 11,
        }}
      >
        <div style={{ display: "flex", gap: 11 }}>
          <div style={{ flex: 1 }}>
            <div
              style={{
                fontSize: 12.5,
                fontWeight: 600,
                color: "var(--txt3)",
                marginBottom: 5,
              }}
            >
              Label
            </div>
            <input
              data-testid="host-add-label"
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder="gpu-box-2"
              className="mono"
              style={{
                width: "100%",
                background: "var(--panel)",
                border: "1px solid var(--border2)",
                borderRadius: 8,
                padding: "9px 11px",
                color: "var(--txt)",
                fontSize: 14.5,
                outline: "none",
                boxSizing: "border-box",
                fontFamily: "var(--font-mono)",
              }}
            />
          </div>
          <div style={{ flex: 1.6 }}>
            <div
              style={{
                fontSize: 12.5,
                fontWeight: 600,
                color: "var(--txt3)",
                marginBottom: 5,
              }}
            >
              SSH target
            </div>
            <input
              data-testid="host-add-target"
              value={target}
              onChange={(e) => setTarget(e.target.value)}
              placeholder="user@10.0.0.5"
              className="mono"
              style={{
                width: "100%",
                background: "var(--panel)",
                border: "1px solid var(--border2)",
                borderRadius: 8,
                padding: "9px 11px",
                color: "var(--txt)",
                fontSize: 14.5,
                outline: "none",
                boxSizing: "border-box",
                fontFamily: "var(--font-mono)",
              }}
            />
          </div>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <span
            className="mono"
            style={{ fontSize: 12.5, color: "var(--txt3)", flex: 1 }}
          >
            Honors your ~/.ssh/config · installs a small helper on first connect · no inbound ports
          </span>
          <button
            data-testid="host-add-btn"
            disabled={!label.trim() || !target.trim()}
            onClick={handleAdd}
            style={{
              fontFamily: "inherit",
              fontWeight: 700,
              fontSize: 14.5,
              padding: "8px 15px",
              borderRadius: 8,
              border: "none",
              background: !label.trim() || !target.trim() ? "var(--border2)" : "var(--accent)",
              color: "#fff",
              cursor: !label.trim() || !target.trim() ? "default" : "pointer",
              opacity: !label.trim() || !target.trim() ? 0.55 : 1,
            }}
          >
            Add &amp; connect
          </button>
          {added && (
            <span style={{ fontSize: 13.5, color: "var(--done)" }}>
              Queued (backend command pending)
            </span>
          )}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Session defaults tab
// ---------------------------------------------------------------------------

// Shared model catalogue, adapted to this tab's `key` field name.
const MODELS = MODEL_LIST.map((m) => ({ key: m.id, label: m.label }));

const POSTURES: Array<{ key: "ask" | "auto" | "full"; label: string; desc: string }> = [
  {
    key: "ask",
    label: "Ask every time",
    desc: "You'll be prompted before any side-effecting action.",
  },
  {
    key: "auto",
    label: "Auto-approve safe actions",
    desc: "Reads & edits run freely; commands and writes still ask.",
  },
  {
    key: "full",
    label: "Full autonomy",
    desc: "Act without asking — sandboxes only.",
  },
];

function SessionDefaultsTab() {
  // NOTE: backend command pending for persisting session defaults
  const [model, setModel] = useState(DEFAULT_MODEL);
  const [posture, setPosture] = useState<"ask" | "auto" | "full">("ask");

  return (
    <div style={{ maxWidth: 860, display: "flex", flexDirection: "column", gap: 22 }}>
      {/* Default model */}
      <div>
        <div style={S.sectionLabelBold}>Default model</div>
        <div style={{ display: "flex", gap: 8 }}>
          {MODELS.map((m) => {
            const active = model === m.key;
            return (
              <button
                key={m.key}
                data-testid={`defaults-model-${m.key}`}
                onClick={() => setModel(m.key)}
                style={{
                  fontFamily: "inherit",
                  fontSize: 14.5,
                  fontWeight: 600,
                  padding: "9px 14px",
                  borderRadius: 9,
                  cursor: "pointer",
                  border: active ? "1px solid var(--accent)" : "1px solid var(--border2)",
                  background: active ? "rgba(91,141,239,0.12)" : "var(--panel2)",
                  color: active ? "var(--accent2)" : "var(--txt2)",
                  transition: "all 0.15s",
                }}
              >
                {m.label}
              </button>
            );
          })}
        </div>
      </div>

      {/* Default permission posture */}
      <div>
        <div style={S.sectionLabelBold}>Default permission posture</div>
        <div style={{ display: "flex", flexDirection: "column", gap: 7 }}>
          {POSTURES.map((p) => {
            const active = posture === p.key;
            return (
              <div
                key={p.key}
                data-testid={`defaults-posture-${p.key}`}
                onClick={() => setPosture(p.key)}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 11,
                  padding: "12px 14px",
                  borderRadius: 10,
                  cursor: "pointer",
                  border: active ? "1px solid var(--accent)" : "1px solid var(--border2)",
                  background: active ? "rgba(91,141,239,0.12)" : "var(--panel2)",
                  transition: "all 0.15s",
                }}
              >
                <span
                  style={{
                    width: 16,
                    height: 16,
                    borderRadius: "50%",
                    border: `2px solid ${active ? "var(--accent)" : "var(--border2)"}`,
                    flexShrink: 0,
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "center",
                  }}
                >
                  <span
                    style={{
                      width: 7,
                      height: 7,
                      borderRadius: "50%",
                      background: active ? "var(--accent)" : "transparent",
                    }}
                  />
                </span>
                <div>
                  <div style={{ fontWeight: 600, fontSize: 15, color: "var(--txt)" }}>
                    {p.label}
                  </div>
                  <div style={{ fontSize: 13.5, color: "var(--txt3)" }}>{p.desc}</div>
                </div>
              </div>
            );
          })}
        </div>
      </div>

      <div
        className="mono"
        style={{ fontSize: 13, color: "var(--txt3)" }}
      >
        New Session can override any of these per launch.
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
  const [startMarsad, setStartMarsad] = useState(false);

  // Initialize autostart toggle from backend on mount
  useEffect(() => {
    getAutostart()
      .then((enabled) => setStartMarsad(enabled))
      .catch((err) => console.error("[Settings] getAutostart failed:", err));
  }, []);

  return (
    <div style={{ maxWidth: 860, display: "flex", flexDirection: "column", gap: 22 }}>
      {/* Appearance */}
      <div>
        <div style={S.sectionLabelBold}>Appearance</div>
        <div style={{ display: "flex", gap: 8 }}>
          {(["dark", "light"] as const).map((t) => {
            const active = theme === t;
            return (
              <button
                key={t}
                data-testid={`general-theme-${t}`}
                onClick={() => toggleTheme()}
                style={{
                  fontFamily: "inherit",
                  fontSize: 14.5,
                  fontWeight: 600,
                  padding: "9px 18px",
                  borderRadius: 9,
                  cursor: "pointer",
                  border: active ? "1px solid var(--accent)" : "1px solid var(--border2)",
                  background: active ? "rgba(91,141,239,0.12)" : "var(--panel2)",
                  color: active ? "var(--accent2)" : "var(--txt2)",
                  textTransform: "capitalize",
                  transition: "all 0.15s",
                }}
              >
                {t.charAt(0).toUpperCase() + t.slice(1)}
              </button>
            );
          })}
        </div>
      </div>

      {/* Background */}
      <div>
        <div style={S.sectionLabelBold}>Background</div>
        <div style={{ display: "flex", gap: 8, marginBottom: 10 }}>
          {BG_OPTIONS.map((bg) => {
            const active = background === bg;
            return (
              <button
                key={bg}
                data-testid={`general-bg-${bg.toLowerCase().replace(/\s+/g, "-")}`}
                onClick={() => setBackground(bg)}
                style={{
                  fontFamily: "inherit",
                  fontSize: 14.5,
                  fontWeight: 600,
                  padding: "9px 16px",
                  borderRadius: 9,
                  cursor: "pointer",
                  border: active ? "1px solid var(--accent)" : "1px solid var(--border2)",
                  background: active ? "rgba(91,141,239,0.12)" : "var(--panel2)",
                  color: active ? "var(--accent2)" : "var(--txt2)",
                  transition: "all 0.15s",
                }}
              >
                {bg}
              </button>
            );
          })}
        </div>
        {background === "Custom image" && (
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 12,
              padding: "12px 14px",
              borderRadius: 11,
              border: "1px dashed var(--border2)",
              background: "var(--panel2)",
            }}
          >
            <div
              style={{
                width: 48,
                height: 32,
                borderRadius: 6,
                background: "var(--border2)",
                backgroundSize: "cover",
                backgroundPosition: "center",
                border: "1px solid var(--border2)",
                flexShrink: 0,
              }}
            />
            <div style={{ flex: 1, fontSize: 14, color: "var(--txt3)" }}>
              No image selected
            </div>
            <button
              style={{
                fontFamily: "inherit",
                fontWeight: 600,
                fontSize: 14,
                padding: "7px 13px",
                borderRadius: 8,
                border: "1px solid var(--border2)",
                background: "var(--panel)",
                color: "var(--txt2)",
                cursor: "pointer",
              }}
            >
              Upload image…
            </button>
          </div>
        )}
        {background === "Desktop" && (
          <div
            style={{
              display: "flex",
              alignItems: "flex-start",
              gap: 8,
              padding: "11px 13px",
              borderRadius: 10,
              background: "var(--panel2)",
              border: "1px solid var(--border)",
              fontSize: 13.5,
              color: "var(--txt3)",
              lineHeight: 1.45,
            }}
          >
            <span style={{ flexShrink: 0 }}>ⓘ</span>
            <span>
              Blurs your live desktop where the compositor supports window translucency (KWin,
              Mutter, picom…). Falls back to the gradient automatically where it doesn't.
            </span>
          </div>
        )}
        <div style={{ fontSize: 13, color: "var(--txt3)", marginTop: 8 }}>
          Shows through translucent surfaces — enable "Translucent surfaces (glass)" below to see
          it.
        </div>
      </div>

      {/* Behavior */}
      <div>
        <div style={S.sectionLabelBold}>Behavior</div>
        <div style={{ display: "flex", flexDirection: "column", gap: 9 }}>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 13,
              padding: "13px 15px",
              borderRadius: 11,
              background: "var(--panel)",
              border: "1px solid var(--border)",
            }}
          >
            <div style={{ flex: 1, minWidth: 0 }}>
              <div style={{ fontWeight: 600, fontSize: 15.5, color: "var(--txt)" }}>
                Start Marsad on login
              </div>
              <div style={{ fontSize: 14, color: "var(--txt3)", marginTop: 1 }}>
                Keep the app ready and reattach to running sessions.
              </div>
            </div>
            <div data-testid="general-autostart-toggle">
              <Toggle
                checked={startMarsad}
                onChange={(next) => {
                  setStartMarsad(next);
                  setAutostart(next).catch((err) => {
                    console.error("[Settings] setAutostart failed:", err);
                    setStartMarsad(!next);
                  });
                }}
              />
            </div>
          </div>

          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 13,
              padding: "13px 15px",
              borderRadius: 11,
              background: "var(--panel)",
              border: "1px solid var(--border)",
            }}
          >
            <div style={{ flex: 1, minWidth: 0 }}>
              <div style={{ fontWeight: 600, fontSize: 15.5, color: "var(--txt)" }}>
                Restore workspace on launch
              </div>
              <div style={{ fontSize: 14, color: "var(--txt3)", marginTop: 1 }}>
                Re-open the workspace and active filters from last time.
              </div>
            </div>
            <div data-testid="general-restore-toggle-wrap">
              <Toggle checked={restoreOnLaunch} onChange={setRestoreOnLaunch} />
            </div>
          </div>

          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 13,
              padding: "13px 15px",
              borderRadius: 11,
              background: "var(--panel)",
              border: "1px solid var(--border)",
            }}
          >
            <div style={{ flex: 1, minWidth: 0 }}>
              <div style={{ fontWeight: 600, fontSize: 15.5, color: "var(--txt)" }}>
                Confirm before stopping a session
              </div>
              <div style={{ fontSize: 14, color: "var(--txt3)", marginTop: 1 }}>
                Ask before you stop any session.
              </div>
            </div>
            <div data-testid="general-confirm-toggle-wrap">
              <Toggle checked={confirmBeforeStop} onChange={setConfirmBeforeStop} />
            </div>
          </div>

          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 13,
              padding: "13px 15px",
              borderRadius: 11,
              background: "var(--panel)",
              border: "1px solid var(--border)",
            }}
          >
            <div style={{ flex: 1, minWidth: 0 }}>
              <div style={{ fontWeight: 600, fontSize: 15.5, color: "var(--txt)" }}>
                Translucent surfaces (glass)
              </div>
              <div style={{ fontSize: 14, color: "var(--txt3)", marginTop: 1 }}>
                Frosted, semi-transparent panels over a backdrop.
              </div>
            </div>
            <div data-testid="general-glass-toggle-wrap">
              <Toggle checked={glass} onChange={toggleGlass} />
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
        <div style={S.titleRow}>
          <h1 style={S.title}>Settings</h1>
          <span style={S.titleTab}>{activeTab}</span>
        </div>
        <p style={S.subtitle}>{TAB_SUBTITLES[activeTab]}</p>
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
        <div style={S.bodyInner}>
          {activeTab === "Notifications" && <NotificationsTab />}
          {activeTab === "Hosts" && <HostsTab />}
          {activeTab === "Session defaults" && <SessionDefaultsTab />}
          {activeTab === "General" && <GeneralTab />}
        </div>
      </div>
    </div>
  );
}
