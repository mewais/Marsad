/**
 * Workspaces screen — switcher pill + manager.
 *
 * All mutations (save / duplicate / delete / export / import / set-active) are
 * wired to real backend commands.  The screen is fully live: the vm-update
 * "workspaces" event replaces the list whenever the backend changes it.
 */

import React, { useRef, useState } from "react";
import { useVm } from "../bridge/useVm";
import {
  setWorkspace,
  saveWorkspace,
  duplicateWorkspace,
  deleteWorkspace,
  exportWorkspace,
  importWorkspace,
} from "../bridge/api";
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
  onDuplicate: (id: string) => void;
  onExport: (id: string) => void;
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
          onClick={() => onDuplicate(ws.id)}
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
          onClick={() => onExport(ws.id)}
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
  const { workspaces, activeWs, dispatch, state } = useVm();
  const [autoSave, setAutoSave] = useState(false);
  const [saveAsName, setSaveAsName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const importRef = useRef<HTMLInputElement>(null);

  function handleLoad(id: string) {
    setWorkspace(id);
    dispatch({ type: "SET_WORKSPACE", id });
  }

  function handleDuplicate(id: string) {
    setError(null);
    void Promise.resolve(duplicateWorkspace(id)).catch((e: unknown) =>
      setError(typeof e === "string" ? e : String(e))
    );
  }

  function handleExport(id: string) {
    setError(null);
    void Promise.resolve(exportWorkspace(id))
      .then((json) => {
        const ws = workspaces.find((w) => w.id === id);
        const filename = `${(ws?.name ?? id).replace(/\s+/g, "_")}.marsad.json`;
        const blob = new Blob([json ?? "{}"], { type: "application/json" });
        const url = URL.createObjectURL(blob);
        const a = document.createElement("a");
        a.href = url;
        a.download = filename;
        a.click();
        URL.revokeObjectURL(url);
      })
      .catch((e: unknown) => setError(typeof e === "string" ? e : String(e)));
  }

  function handleDelete(id: string) {
    setError(null);
    void Promise.resolve(deleteWorkspace(id)).catch((e: unknown) =>
      setError(typeof e === "string" ? e : String(e))
    );
  }

  function handleSaveAs() {
    if (!saveAsName.trim()) return;
    setError(null);
    const currentSessionIds =
      state.snapshot?.sessions.map((s) => s.id) ?? [];
    void Promise.resolve(saveWorkspace(saveAsName.trim(), currentSessionIds))
      .then(() => setSaveAsName(""))
      .catch((e: unknown) => setError(typeof e === "string" ? e : String(e)));
  }

  function handleImport(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    setError(null);
    const reader = new FileReader();
    reader.onload = (ev) => {
      const text = ev.target?.result as string;
      void Promise.resolve(importWorkspace(text)).catch((err: unknown) =>
        setError(typeof err === "string" ? err : String(err))
      );
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

      {/* Error banner */}
      {error && (
        <div
          style={{
            padding: "8px 22px",
            background: "var(--error-bg, rgba(224,89,74,0.12))",
            borderBottom: "1px solid var(--border)",
            color: "var(--error)",
            fontSize: 13,
          }}
        >
          {error}
        </div>
      )}

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
        {workspaces.map((ws) => (
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
        {workspaces.length === 0 && (
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
