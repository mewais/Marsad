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
import { Button } from "../components/Button";
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
// Workspace row (in manager modal)
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
        display: "flex",
        alignItems: "center",
        gap: 10,
        padding: "12px 20px",
        borderBottom: "1px solid var(--border)",
        background: isActive ? "var(--accent-bg)" : "transparent",
        transition: "background 0.15s",
      }}
    >
      {/* Active dot */}
      <span
        style={{
          width: 8,
          height: 8,
          borderRadius: "50%",
          background: isActive ? "var(--accent)" : "var(--border2)",
          flexShrink: 0,
        }}
      />

      {/* Info */}
      <div style={{ flex: 1, minWidth: 0 }}>
        <div
          style={{
            fontSize: 13,
            fontWeight: 600,
            color: isActive ? "var(--accent2)" : "var(--txt)",
            display: "flex",
            alignItems: "center",
            gap: 6,
          }}
        >
          {ws.name}
          {isActive && (
            <span
              style={{
                fontSize: 9,
                fontWeight: 700,
                letterSpacing: 0.4,
                padding: "1px 5px",
                borderRadius: 4,
                background: "var(--accent)",
                color: "#fff",
                textTransform: "uppercase",
              }}
            >
              active
            </span>
          )}
        </div>
        <div style={{ fontSize: 11, color: "var(--txt3)", marginTop: 2 }}>
          {fmtSessions(ws.session_ids)} · saved {ws.saved_at}
        </div>
      </div>

      {/* Actions */}
      <div style={{ display: "flex", gap: 6, flexShrink: 0 }}>
        <Button
          variant="primary"
          size="sm"
          data-testid={`ws-load-${ws.id}`}
          onClick={() => onLoad(ws.id)}
          disabled={isActive}
          style={{ opacity: isActive ? 0.4 : 1 }}
        >
          Load
        </Button>
        <Button
          variant="secondary"
          size="sm"
          data-testid={`ws-duplicate-${ws.id}`}
          onClick={() => onDuplicate(ws)}
        >
          Duplicate
        </Button>
        <Button
          variant="ghost"
          size="sm"
          data-testid={`ws-export-${ws.id}`}
          onClick={() => onExport(ws)}
        >
          Export
        </Button>
        <Button
          variant="danger"
          size="sm"
          data-testid={`ws-delete-${ws.id}`}
          onClick={() => onDelete(ws.id)}
          disabled={isActive}
          style={{ opacity: isActive ? 0.35 : 1 }}
        >
          Delete
        </Button>
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
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "18px 20px 12px",
          borderBottom: "1px solid var(--border)",
        }}
      >
        <div style={{ fontSize: 16, fontWeight: 700, color: "var(--txt)" }}>
          Workspace manager
        </div>
        <button
          onClick={onClose}
          style={{
            background: "transparent",
            border: "none",
            color: "var(--txt3)",
            fontSize: 18,
            cursor: "pointer",
            lineHeight: 1,
          }}
        >
          ×
        </button>
      </div>

      {/* Auto-save toggle */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "12px 20px",
          borderBottom: "1px solid var(--border)",
          background: "var(--panel2)",
        }}
      >
        <div>
          <div style={{ fontSize: 13, fontWeight: 500, color: "var(--txt)" }}>
            Auto-save workspace
          </div>
          <div style={{ fontSize: 11, color: "var(--txt3)", marginTop: 2 }}>
            Automatically snapshot sessions on change
            {/* NOTE: auto-save backend command pending */}
          </div>
        </div>
        <div data-testid="ws-autosave-toggle-wrap">
          <Toggle checked={autoSave} onChange={setAutoSave} />
        </div>
      </div>

      {/* Workspace list */}
      <div style={{ maxHeight: 320, overflowY: "auto" }}>
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
              padding: "32px 20px",
              textAlign: "center",
              color: "var(--txt3)",
              fontSize: 13,
            }}
          >
            No workspaces yet. Save the current state below.
          </div>
        )}
      </div>

      {/* Save current as */}
      <div
        style={{
          padding: "14px 20px",
          borderTop: "1px solid var(--border)",
          display: "flex",
          gap: 8,
          alignItems: "center",
        }}
      >
        <input
          data-testid="ws-save-as-input"
          value={saveAsName}
          onChange={(e) => setSaveAsName(e.target.value)}
          placeholder="Save current as…"
          style={{
            flex: 1,
            background: "var(--panel2)",
            border: "1px solid var(--border2)",
            borderRadius: 7,
            color: "var(--txt)",
            fontSize: 13,
            padding: "8px 10px",
            fontFamily: "inherit",
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") handleSaveAs();
          }}
        />
        <Button
          data-testid="ws-save-as-btn"
          variant="primary"
          size="sm"
          disabled={!saveAsName.trim()}
          onClick={handleSaveAs}
        >
          Save
        </Button>
        {/* Import */}
        <input
          ref={importRef}
          type="file"
          accept=".marsad.json,application/json"
          style={{ display: "none" }}
          onChange={handleImport}
        />
        <Button
          data-testid="ws-import-btn"
          variant="ghost"
          size="sm"
          onClick={() => importRef.current?.click()}
        >
          Import
        </Button>
      </div>
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// Workspace summary row (in main screen list)
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
        padding: "12px 16px",
        background: isActive ? "var(--accent-bg)" : "transparent",
      }}
    >
      <span
        style={{
          width: 8,
          height: 8,
          borderRadius: "50%",
          background: isActive ? "var(--accent)" : "var(--border2)",
          flexShrink: 0,
        }}
      />
      <div style={{ flex: 1 }}>
        <div
          style={{
            fontSize: 13,
            fontWeight: 600,
            color: isActive ? "var(--accent2)" : "var(--txt)",
          }}
        >
          {ws.name}
        </div>
        <div style={{ fontSize: 11, color: "var(--txt3)", marginTop: 2 }}>
          {fmtSessions(ws.session_ids)} · {ws.saved_at}
        </div>
      </div>
      <Button
        variant="primary"
        size="sm"
        data-testid={`ws-load-summary-${ws.id}`}
        disabled={isActive}
        style={{ opacity: isActive ? 0.4 : 1 }}
        onClick={() => onLoad(ws.id)}
      >
        {isActive ? "Active" : "Load"}
      </Button>
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
              gap: 6,
              padding: "6px 14px",
              borderRadius: 20,
              border: isActive ? "1px solid var(--accent)" : "1px solid var(--border2)",
              background: isActive ? "var(--accent-bg)" : "var(--panel2)",
              color: isActive ? "var(--accent2)" : "var(--txt2)",
              fontWeight: isActive ? 700 : 500,
              fontSize: 12.5,
              cursor: isActive ? "default" : "pointer",
              fontFamily: "inherit",
              transition: "all 0.15s",
            }}
          >
            {isActive && (
              <span
                style={{
                  width: 6,
                  height: 6,
                  borderRadius: "50%",
                  background: "var(--accent)",
                  display: "inline-block",
                }}
              />
            )}
            {ws.name}
            <span
              style={{
                fontSize: 10,
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
      style={{ padding: "22px 26px", color: "var(--txt)", height: "100%", overflowY: "auto" }}
    >
      {/* Header */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          marginBottom: 20,
        }}
      >
        <h1
          style={{
            fontSize: 21,
            fontWeight: 700,
            margin: 0,
            letterSpacing: -0.3,
          }}
        >
          Workspaces
        </h1>
        <Button
          data-testid="ws-open-manager"
          variant="secondary"
          size="sm"
          onClick={() => setManagerOpen(true)}
        >
          Manage…
        </Button>
      </div>

      {/* Active workspace info */}
      {activeWs && (
        <div
          data-testid="ws-active-info"
          style={{
            padding: "14px 16px",
            borderRadius: 10,
            border: "1px solid var(--accent)",
            background: "var(--accent-bg)",
            marginBottom: 20,
          }}
        >
          <div
            style={{
              fontSize: 11,
              fontWeight: 700,
              letterSpacing: 0.5,
              textTransform: "uppercase",
              color: "var(--accent)",
              marginBottom: 4,
            }}
          >
            Active workspace
          </div>
          <div style={{ fontSize: 15, fontWeight: 700, color: "var(--txt)" }}>
            {activeWs.name}
          </div>
          <div style={{ fontSize: 11, color: "var(--txt3)", marginTop: 4 }}>
            {fmtSessions(activeWs.session_ids)} · saved {activeWs.saved_at}
          </div>
        </div>
      )}

      {/* Switcher */}
      <div style={{ marginBottom: 24 }}>
        <div
          style={{
            fontSize: 11,
            fontWeight: 700,
            letterSpacing: 0.8,
            textTransform: "uppercase",
            color: "var(--txt3)",
            marginBottom: 10,
          }}
        >
          Switch workspace
        </div>
        <WsSwitcher />
      </div>

      {/* Workspace list summary */}
      <div>
        <div
          style={{
            fontSize: 11,
            fontWeight: 700,
            letterSpacing: 0.8,
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
            borderRadius: 10,
            overflow: "hidden",
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
        </div>
      </div>

      {/* Manager modal */}
      <WsManagerModal open={managerOpen} onClose={() => setManagerOpen(false)} />
    </div>
  );
}
