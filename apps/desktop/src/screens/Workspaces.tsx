/**
 * Workspaces screen — switcher pill + manager.
 *
 * Workspace switcher: shows all workspaces; active highlighted; Load calls
 * setWorkspace (api) + dispatches SET_WORKSPACE to the store.
 *
 * Manager modal: per-workspace row with Load / Duplicate / Export /  Delete.
 *   - Load → setWorkspace (api) + dispatch SET_WORKSPACE
 *   - Duplicate → local state only. NOTE: backend command pending.
 *   - Export → triggers JSON download (client-side only). NOTE: backend pending.
 *   - Delete → local state only. NOTE: backend command for deleteWorkspace pending.
 *   - "Save current as" → local state only. NOTE: backend saveWorkspace pending.
 *
 * Import: reads a .marsad.json file client-side. NOTE: backend import command pending.
 * Auto-save toggle: local state. NOTE: backend command pending.
 */

import React, { useRef, useState } from "react";
import { useVm } from "../bridge/useVm";
import { setWorkspace } from "../bridge/api";
import type { Workspace } from "../bridge/types";
import { Toggle } from "../components/Toggle";
import { Modal } from "../components/Modal";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function fmtSessions(ids: string[]): string {
  const n = ids.length;
  return `${n} session${n !== 1 ? "s" : ""}`;
}

// ---------------------------------------------------------------------------
// Workspace card (in manager modal)
// ---------------------------------------------------------------------------

interface WsRowProps {
  ws: Workspace;
  isActive: boolean;
  onLoad: (id: string) => void;
  onDuplicate: (ws: Workspace) => void;
  onExport: (ws: Workspace) => void;
  onDelete: (id: string) => void;
}

function WsManagerRow({
  ws,
  isActive,
  onLoad,
  onDuplicate,
  onExport,
  onDelete,
}: WsRowProps) {
  return (
    <div
      data-testid={`ws-row-${ws.id}`}
      style={{
        borderRadius: 13,
        border: `1px solid ${isActive ? "var(--accent)" : "var(--border)"}`,
        background: "var(--panel)",
        padding: 15,
        position: "relative",
        transition: "border-color 0.15s",
      }}
    >
      {/* Info header */}
      <div style={{ display: "flex", alignItems: "flex-start", gap: 10 }}>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <span
              style={{
                fontWeight: 700,
                fontSize: 17,
                color: "var(--txt)",
              }}
            >
              {ws.name}
            </span>
            {isActive && (
              <span
                style={{
                  fontSize: 11.5,
                  fontWeight: 700,
                  letterSpacing: 0.3,
                  textTransform: "uppercase",
                  color: "var(--accent2)",
                  background: "var(--accent-bg)",
                  padding: "1px 7px",
                  borderRadius: 5,
                }}
              >
                Active
              </span>
            )}
          </div>
          <div
            className="mono"
            style={{
              fontSize: 13,
              color: "var(--txt3)",
              marginTop: 3,
            }}
          >
            {fmtSessions(ws.session_ids)} · saved {ws.saved_at}
          </div>
        </div>
      </div>

      {/* Session chips — derived from session_ids as placeholder labels */}
      <div
        style={{
          display: "flex",
          gap: 6,
          flexWrap: "wrap",
          margin: "11px 0 13px",
        }}
      >
        {ws.session_ids.slice(0, 3).map((sid) => (
          <span
            key={sid}
            style={{
              fontSize: 13,
              padding: "3px 9px",
              borderRadius: 6,
              background: "var(--panel2)",
              border: "1px solid var(--border)",
              color: "var(--txt2)",
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
              maxWidth: 150,
            }}
          >
            {sid}
          </span>
        ))}
        {ws.session_ids.length > 3 && (
          <span
            style={{
              fontSize: 13,
              padding: "3px 9px",
              borderRadius: 6,
              color: "var(--txt3)",
            }}
          >
            {ws.session_ids.length - 3} more
          </span>
        )}
      </div>

      {/* Actions */}
      <div style={{ display: "flex", gap: 7, alignItems: "center" }}>
        <button
          data-testid={`ws-load-${ws.id}`}
          onClick={() => onLoad(ws.id)}
          disabled={isActive}
          style={{
            fontFamily: "inherit",
            fontWeight: 700,
            fontSize: 14,
            padding: "7px 14px",
            borderRadius: 8,
            border: "none",
            background: isActive ? "var(--border2)" : "var(--accent)",
            color: "#fff",
            cursor: isActive ? "default" : "pointer",
            opacity: isActive ? 0.5 : 1,
            transition: "opacity 0.15s",
          }}
        >
          Load
        </button>
        <button
          data-testid={`ws-duplicate-${ws.id}`}
          onClick={() => onDuplicate(ws)}
          style={{
            fontFamily: "inherit",
            fontWeight: 600,
            fontSize: 14,
            padding: "7px 12px",
            borderRadius: 8,
            border: "1px solid var(--border2)",
            background: "var(--panel2)",
            color: "var(--txt2)",
            cursor: "pointer",
          }}
        >
          Duplicate
        </button>
        <button
          data-testid={`ws-export-${ws.id}`}
          onClick={() => onExport(ws)}
          style={{
            fontFamily: "inherit",
            fontWeight: 600,
            fontSize: 14,
            padding: "7px 12px",
            borderRadius: 8,
            border: "1px solid var(--border2)",
            background: "var(--panel2)",
            color: "var(--txt2)",
            cursor: "pointer",
          }}
        >
          ↧ Export
        </button>
        <span style={{ flex: 1 }} />
        <button
          data-testid={`ws-delete-${ws.id}`}
          onClick={() => onDelete(ws.id)}
          disabled={isActive}
          style={{
            fontFamily: "inherit",
            fontWeight: 600,
            fontSize: 14,
            padding: "7px 12px",
            borderRadius: 8,
            border: "1px solid var(--border2)",
            background: "transparent",
            color: "var(--error)",
            cursor: isActive ? "default" : "pointer",
            opacity: isActive ? 0.35 : 1,
            transition: "opacity 0.15s",
          }}
        >
          Delete
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Manager modal
// ---------------------------------------------------------------------------

interface WsManagerModalProps {
  open: boolean;
  onClose: () => void;
}

function WsManagerModal({ open, onClose }: WsManagerModalProps) {
  const { workspaces, activeWs, dispatch } = useVm();
  // NOTE: autoSave local state — backend command pending
  const [autoSave, setAutoSave] = useState(false);
  // NOTE: saveAsName local state — backend saveWorkspace command pending
  const [saveAsName, setSaveAsName] = useState("");
  const importRef = useRef<HTMLInputElement>(null);

  // Local copy of workspaces for client-side duplicate/delete
  // NOTE: all mutations (duplicate, delete) are local only — backend commands pending
  const [localWs, setLocalWs] = useState<Workspace[]>([]);
  // We use the store workspaces + any local additions
  const allWs = [...workspaces, ...localWs];

  function handleLoad(id: string) {
    setWorkspace(id);
    dispatch({ type: "SET_WORKSPACE", id });
  }

  function handleDuplicate(ws: Workspace) {
    // NOTE: backend duplicateWorkspace command pending
    const dup: Workspace = {
      id: `ws-dup-${Date.now()}`,
      name: `${ws.name} (copy)`,
      session_ids: [...ws.session_ids],
      saved_at: "just now",
    };
    setLocalWs((prev) => [...prev, dup]);
  }

  function handleExport(ws: Workspace) {
    // NOTE: backend exportWorkspace command pending — client-side download only
    const blob = new Blob([JSON.stringify(ws, null, 2)], {
      type: "application/json",
    });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `${ws.name.replace(/\s+/g, "_")}.marsad.json`;
    a.click();
    URL.revokeObjectURL(url);
  }

  function handleDelete(id: string) {
    // NOTE: backend deleteWorkspace command pending
    setLocalWs((prev) => prev.filter((w) => w.id !== id));
    // Can't delete store workspaces without backend — show no-op
  }

  function handleSaveAs() {
    if (!saveAsName.trim()) return;
    // NOTE: backend saveWorkspace command pending
    const newWs: Workspace = {
      id: `ws-saved-${Date.now()}`,
      name: saveAsName.trim(),
      session_ids: activeWs?.session_ids ?? [],
      saved_at: "just now",
    };
    setLocalWs((prev) => [...prev, newWs]);
    setSaveAsName("");
  }

  function handleImport(e: React.ChangeEvent<HTMLInputElement>) {
    // NOTE: backend importWorkspace command pending — client-side JSON parse only
    const file = e.target.files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = (ev) => {
      try {
        const ws = JSON.parse(ev.target?.result as string) as Workspace;
        setLocalWs((prev) => [
          ...prev,
          { ...ws, id: `ws-import-${Date.now()}`, saved_at: "just now" },
        ]);
      } catch {
        // ignore parse errors
      }
    };
    reader.readAsText(file);
    e.target.value = "";
  }

  return (
    <Modal open={open} onClose={onClose} width={680}>
      {/* Modal header */}
      <div
        style={{
          padding: "18px 22px",
          borderBottom: "1px solid var(--border)",
          display: "flex",
          alignItems: "center",
          gap: 12,
        }}
      >
        {/* Icon box */}
        <div
          style={{
            width: 34,
            height: 34,
            borderRadius: 9,
            background: "var(--accent-bg)",
            border: "1px solid var(--border2)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            color: "var(--accent2)",
            fontSize: 19,
            flexShrink: 0,
          }}
        >
          ⛶
        </div>

        {/* Title + subtitle */}
        <div style={{ flex: 1 }}>
          <h2
            style={{
              fontSize: 19,
              fontWeight: 700,
              margin: 0,
              letterSpacing: -0.2,
              color: "var(--txt)",
            }}
          >
            Workspaces
          </h2>
          <div style={{ fontSize: 14, color: "var(--txt3)", marginTop: 1 }}>
            Saved snapshots of your sessions, history and state — switch any time
          </div>
        </div>

        {/* Import button */}
        <input
          ref={importRef}
          type="file"
          accept=".marsad.json,application/json"
          style={{ display: "none" }}
          onChange={handleImport}
        />
        <button
          data-testid="ws-import-btn"
          onClick={() => importRef.current?.click()}
          style={{
            fontFamily: "inherit",
            fontWeight: 600,
            fontSize: 14.5,
            padding: "8px 13px",
            borderRadius: 8,
            border: "1px solid var(--border2)",
            background: "var(--panel2)",
            color: "var(--txt2)",
            cursor: "pointer",
          }}
        >
          ↥ Import file
        </button>

        {/* Close button */}
        <button
          onClick={onClose}
          style={{
            width: 30,
            height: 30,
            borderRadius: 8,
            border: "1px solid var(--border)",
            background: "var(--panel2)",
            color: "var(--txt2)",
            cursor: "pointer",
            fontSize: 17,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          ✕
        </button>
      </div>

      {/* Auto-save toggle */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "10px 22px",
          borderBottom: "1px solid var(--border)",
          background: "var(--panel2)",
        }}
      >
        <div>
          <div style={{ fontSize: 15, fontWeight: 600, color: "var(--txt)" }}>
            Auto-save workspace
          </div>
          <div style={{ fontSize: 13, color: "var(--txt3)", marginTop: 1 }}>
            Automatically snapshot sessions on change
          </div>
        </div>
        <div data-testid="ws-autosave-toggle-wrap">
          <Toggle checked={autoSave} onChange={setAutoSave} />
        </div>
      </div>

      {/* Workspace cards */}
      <div
        style={{
          flex: 1,
          overflowY: "auto",
          padding: "16px 22px",
          display: "flex",
          flexDirection: "column",
          gap: 11,
        }}
      >
        {allWs.map((ws) => (
          <WsManagerRow
            key={ws.id}
            ws={ws}
            isActive={activeWs?.id === ws.id}
            onLoad={handleLoad}
            onDuplicate={handleDuplicate}
            onExport={handleExport}
            onDelete={handleDelete}
          />
        ))}
        {allWs.length === 0 && (
          <div
            style={{
              padding: "32px 0",
              textAlign: "center",
              color: "var(--txt3)",
              fontSize: 15.5,
            }}
          >
            No workspaces yet. Save the current state below.
          </div>
        )}
      </div>

      {/* Footer: save-as + local note */}
      <div
        style={{
          flexShrink: 0,
          borderTop: "1px solid var(--border)",
        }}
      >
        {/* Save current as */}
        <div
          style={{
            padding: "13px 22px",
            display: "flex",
            gap: 8,
            alignItems: "center",
            borderBottom: "1px solid var(--border)",
          }}
        >
          <input
            data-testid="ws-save-as-input"
            value={saveAsName}
            onChange={(e) => setSaveAsName(e.target.value)}
            placeholder="Save current as…"
            style={{
              flex: 1,
              minWidth: 0,
              background: "var(--panel2)",
              border: "1px solid var(--border2)",
              borderRadius: 8,
              color: "var(--txt)",
              fontSize: 14.5,
              padding: "7px 10px",
              fontFamily: "inherit",
              outline: "none",
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") handleSaveAs();
            }}
          />
          <button
            data-testid="ws-save-as-btn"
            disabled={!saveAsName.trim()}
            onClick={handleSaveAs}
            style={{
              fontFamily: "inherit",
              fontWeight: 700,
              fontSize: 14,
              padding: "7px 12px",
              borderRadius: 8,
              border: "none",
              background: saveAsName.trim() ? "var(--accent)" : "var(--border2)",
              color: "#fff",
              cursor: saveAsName.trim() ? "pointer" : "default",
              opacity: saveAsName.trim() ? 1 : 0.5,
              transition: "background 0.15s, opacity 0.15s",
            }}
          >
            Save
          </button>
        </div>

        {/* Local storage note */}
        <div style={{ padding: "13px 22px" }}>
          <span
            className="mono"
            style={{ fontSize: 12.5, color: "var(--txt3)" }}
          >
            Stored locally · export a workspace to a file to move or share it. Cloud sync is out of scope.
          </span>
        </div>
      </div>
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// Workspace summary card (in main screen list)
// ---------------------------------------------------------------------------

interface WsSummaryRowProps {
  ws: Workspace;
  isActive: boolean;
  onLoad: (id: string) => void;
}

function WsSummaryRow({ ws, isActive, onLoad }: WsSummaryRowProps) {
  return (
    <div
      data-testid={`ws-summary-row-${ws.id}`}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 12,
        padding: "13px 16px",
        background: isActive ? "var(--accent-bg)" : "transparent",
        transition: "background 0.15s",
      }}
    >
      {/* Active indicator dot */}
      <span
        style={{
          width: 14,
          height: 14,
          borderRadius: "50%",
          border: `2px solid ${isActive ? "var(--accent)" : "var(--border2)"}`,
          flexShrink: 0,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
        }}
      >
        <span
          style={{
            width: 6,
            height: 6,
            borderRadius: "50%",
            background: isActive ? "var(--accent)" : "transparent",
          }}
        />
      </span>

      {/* Info */}
      <div style={{ flex: 1, minWidth: 0 }}>
        <div
          style={{
            fontSize: 15,
            fontWeight: 600,
            color: isActive ? "var(--accent2)" : "var(--txt)",
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {ws.name}
        </div>
        <div
          className="mono"
          style={{ fontSize: 12, color: "var(--txt3)", marginTop: 2 }}
        >
          {fmtSessions(ws.session_ids)} · saved {ws.saved_at}
        </div>
      </div>

      {/* Load button */}
      <button
        data-testid={`ws-load-summary-${ws.id}`}
        disabled={isActive}
        onClick={() => onLoad(ws.id)}
        style={{
          fontFamily: "inherit",
          fontWeight: 700,
          fontSize: 14,
          padding: "6px 14px",
          borderRadius: 8,
          border: "none",
          background: isActive ? "var(--border2)" : "var(--accent)",
          color: "#fff",
          cursor: isActive ? "default" : "pointer",
          opacity: isActive ? 0.5 : 1,
          flexShrink: 0,
          transition: "opacity 0.15s",
        }}
      >
        {isActive ? "Active" : "Load"}
      </button>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Workspace switcher pill
// ---------------------------------------------------------------------------

function WsSwitcher() {
  const { workspaces, activeWs, dispatch } = useVm();

  function handleLoad(id: string) {
    setWorkspace(id);
    dispatch({ type: "SET_WORKSPACE", id });
  }

  return (
    <div
      data-testid="ws-switcher"
      style={{
        display: "flex",
        gap: 6,
        flexWrap: "wrap",
      }}
    >
      {workspaces.map((ws) => {
        const isActive = activeWs?.id === ws.id;
        return (
          <button
            key={ws.id}
            data-testid={`ws-pill-${ws.id}`}
            data-active={isActive ? "true" : "false"}
            onClick={() => !isActive && handleLoad(ws.id)}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 7,
              padding: "6px 11px",
              borderRadius: 9,
              border: `1px solid ${isActive ? "var(--accent)" : "var(--border)"}`,
              background: isActive ? "var(--accent-bg)" : "var(--panel2)",
              color: isActive ? "var(--accent2)" : "var(--txt2)",
              fontWeight: isActive ? 700 : 600,
              fontSize: 15,
              cursor: isActive ? "default" : "pointer",
              fontFamily: "inherit",
              transition: "all 0.15s",
            }}
          >
            {/* Ring + dot indicator */}
            <span
              style={{
                width: 14,
                height: 14,
                borderRadius: "50%",
                border: `2px solid ${isActive ? "var(--accent)" : "var(--border2)"}`,
                flexShrink: 0,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
              }}
            >
              <span
                style={{
                  width: 6,
                  height: 6,
                  borderRadius: "50%",
                  background: isActive ? "var(--accent)" : "transparent",
                }}
              />
            </span>

            {/* Name */}
            <span
              style={{
                whiteSpace: "nowrap",
                overflow: "hidden",
                textOverflow: "ellipsis",
                maxWidth: 160,
              }}
            >
              {ws.name}
            </span>

            {/* Session count */}
            <span
              className="mono"
              style={{
                fontSize: 12,
                color: isActive ? "var(--accent)" : "var(--txt3)",
                fontWeight: 500,
              }}
            >
              {fmtSessions(ws.session_ids)}
            </span>
          </button>
        );
      })}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Workspaces screen root
// ---------------------------------------------------------------------------

export function Workspaces() {
  const { workspaces, activeWs, dispatch } = useVm();
  const [managerOpen, setManagerOpen] = useState(false);

  function handleLoadSummary(id: string) {
    setWorkspace(id);
    dispatch({ type: "SET_WORKSPACE", id });
  }

  return (
    <div
      data-testid="workspaces-screen"
      style={{
        padding: "22px 26px",
        color: "var(--txt)",
        height: "100%",
        overflowY: "auto",
      }}
    >
      {/* Header */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          marginBottom: 24,
        }}
      >
        <div>
          <h1
            style={{
              fontSize: 26,
              fontWeight: 700,
              margin: 0,
              letterSpacing: -0.3,
              color: "var(--txt)",
            }}
          >
            Workspaces
          </h1>
          <div style={{ fontSize: 14, color: "var(--txt3)", marginTop: 3 }}>
            Saved snapshots of your sessions, history and state
          </div>
        </div>
        <button
          data-testid="ws-open-manager"
          onClick={() => setManagerOpen(true)}
          style={{
            fontFamily: "inherit",
            fontWeight: 600,
            fontSize: 14.5,
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

      {/* Active workspace info card */}
      {activeWs && (
        <div
          data-testid="ws-active-info"
          style={{
            padding: "13px 15px",
            borderRadius: 12,
            border: "1px solid var(--border2)",
            background: "var(--panel2)",
            marginBottom: 22,
          }}
        >
          <div
            style={{
              fontSize: 12,
              fontWeight: 700,
              letterSpacing: 0.6,
              textTransform: "uppercase",
              color: "var(--txt3)",
              marginBottom: 5,
            }}
          >
            Current workspace
          </div>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              marginBottom: 4,
            }}
          >
            <span
              style={{ fontWeight: 700, fontSize: 17, color: "var(--txt)" }}
            >
              {activeWs.name}
            </span>
            <span style={{ fontSize: 12.5, color: "var(--done)" }}>
              ● auto-saved
            </span>
          </div>
          <div
            style={{
              fontSize: 13,
              color: "var(--txt3)",
              lineHeight: 1.45,
            }}
          >
            {fmtSessions(activeWs.session_ids)} · saved {activeWs.saved_at}
          </div>
        </div>
      )}

      {/* Switch workspace section */}
      <div style={{ marginBottom: 24 }}>
        <div
          style={{
            fontSize: 12,
            fontWeight: 700,
            letterSpacing: 0.6,
            textTransform: "uppercase",
            color: "var(--txt3)",
            marginBottom: 10,
          }}
        >
          Switch to
        </div>
        <WsSwitcher />
      </div>

      {/* All workspaces list */}
      <div>
        <div
          style={{
            fontSize: 12,
            fontWeight: 700,
            letterSpacing: 0.6,
            textTransform: "uppercase",
            color: "var(--txt3)",
            marginBottom: 10,
          }}
        >
          All workspaces ({workspaces.length})
        </div>
        <div
          style={{
            border: "1px solid var(--border)",
            borderRadius: 12,
            overflow: "hidden",
            background: "var(--panel)",
          }}
        >
          {workspaces.map((ws, i) => {
            const isActive = activeWs?.id === ws.id;
            return (
              <div
                key={ws.id}
                style={{
                  borderTop: i === 0 ? "none" : "1px solid var(--border)",
                }}
              >
                <WsSummaryRow
                  ws={ws}
                  isActive={isActive}
                  onLoad={handleLoadSummary}
                />
              </div>
            );
          })}
          {workspaces.length === 0 && (
            <div
              style={{
                padding: "32px 20px",
                textAlign: "center",
                color: "var(--txt3)",
                fontSize: 15.5,
              }}
            >
              No workspaces yet.
            </div>
          )}
        </div>
      </div>

      {/* Manager modal */}
      <WsManagerModal open={managerOpen} onClose={() => setManagerOpen(false)} />
    </div>
  );
}
