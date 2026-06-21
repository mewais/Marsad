/**
 * Changes tab — changed-file list with stage checkboxes + side-by-side diff + commit bar.
 *
 * NOTE: SessionView only exposes a `changed_files` count; it does not carry a
 * per-file list with names, status (A/M/D), or ± line counts. We render a
 * synthetic file list derived from the gate's Edit diff (if present) and stub
 * entries for any remaining count. A richer file-list feed will replace this
 * once the backend exposes a `files` array on SessionView or via a separate
 * Tauri command.
 */
import { useState } from "react";
import { useVm } from "../../bridge/useVm";
import { DiffView } from "../../components/DiffView";
import type { SideRow } from "../../bridge/types";

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

type FileStatus = "A" | "M" | "D";

interface ChangedFile {
  name: string;
  status: FileStatus;
  added: number;
  removed: number;
  diffRows: SideRow[];
}

const STATUS_COLOR: Record<FileStatus, string> = {
  A: "var(--done)",
  M: "var(--accent2)",
  D: "var(--error)",
};

// ---------------------------------------------------------------------------
// Derive synthetic file list from what the snapshot exposes.
// NOTE: Replace with sv.files[] feed when backend exposes it.
// ---------------------------------------------------------------------------

function deriveFiles(
  changedCount: number,
  gate: import("../../bridge/types").GateVM | null
): ChangedFile[] {
  const files: ChangedFile[] = [];

  // If gate is an edit gate, use its file + diff rows
  if (gate && gate.is_edit) {
    files.push({
      name: gate.summary,
      status: "M",
      added: gate.diff_rows.filter((r) => r.r.sig === "+").length,
      removed: gate.diff_rows.filter((r) => r.l.sig === "-").length,
      diffRows: gate.diff_rows,
    });
  }

  // Fill remaining count with stub entries
  const remaining = Math.max(0, changedCount - files.length);
  for (let i = 0; i < remaining; i++) {
    files.push({
      name: `changed-file-${i + 1}.ts`,
      status: i === 0 ? "A" : i % 3 === 0 ? "D" : "M",
      added: Math.floor(Math.random() * 30) + 1,
      removed: Math.floor(Math.random() * 10),
      diffRows: [],
    });
  }

  return files;
}

// ---------------------------------------------------------------------------
// ChangesTab
// ---------------------------------------------------------------------------

export function ChangesTab() {
  const { selectedSession, selectedGate } = useVm();
  const [staged, setStaged] = useState<Record<string, boolean>>({});
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [commitMsg, setCommitMsg] = useState("");

  if (!selectedSession) {
    return (
      <div style={{ padding: 20, color: "var(--txt3)" }}>
        No session selected.
      </div>
    );
  }

  const files = deriveFiles(selectedSession.changed_files, selectedGate);

  // Default all files to staged = true
  const effectiveStaged: Record<string, boolean> = {};
  files.forEach((f) => {
    effectiveStaged[f.name] = staged[f.name] ?? true;
  });

  const activeFile = files.find((f) => f.name === selectedFile) ?? files[0] ?? null;
  const stagedCount = Object.values(effectiveStaged).filter(Boolean).length;

  function toggleStage(name: string) {
    setStaged((prev) => ({ ...prev, [name]: !effectiveStaged[name] }));
  }

  function handleCommit() {
    // NOTE: commit() bridge command will be wired here once the backend
    // exposes it. For now this is a no-op placeholder.
    setCommitMsg("");
  }

  const changeStats = `${files.length} file${files.length !== 1 ? "s" : ""}`;

  return (
    <div style={{ flex: 1, display: "flex", overflow: "hidden" }}>
      {/* File list panel */}
      <div
        style={{
          width: 280,
          flexShrink: 0,
          borderRight: "1px solid var(--border)",
          overflowY: "auto",
          padding: 12,
        }}
      >
        <div
          style={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            marginBottom: 8,
            padding: "0 4px",
          }}
        >
          <span
            style={{
              fontSize: 11,
              fontWeight: 700,
              letterSpacing: 0.5,
              textTransform: "uppercase" as const,
              color: "var(--txt3)",
            }}
          >
            Changed files
          </span>
          <span
            className="mono"
            style={{ fontSize: 10.5, color: "var(--txt3)" }}
          >
            {changeStats}
          </span>
        </div>

        {files.length === 0 ? (
          <div
            style={{
              padding: "20px 10px",
              color: "var(--txt3)",
              fontSize: 11.5,
              textAlign: "center",
            }}
          >
            No changed files
          </div>
        ) : (
          files.map((f) => {
            const isActive = activeFile?.name === f.name;
            return (
              <div
                key={f.name}
                onClick={() => setSelectedFile(f.name)}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 9,
                  padding: "9px 10px",
                  borderRadius: 8,
                  cursor: "pointer",
                  marginBottom: 2,
                  background: isActive ? "var(--accent-bg)" : "transparent",
                  border: isActive
                    ? "1px solid var(--accent)"
                    : "1px solid transparent",
                }}
              >
                <input
                  type="checkbox"
                  checked={effectiveStaged[f.name]}
                  onChange={() => toggleStage(f.name)}
                  onClick={(e) => e.stopPropagation()}
                  style={{
                    accentColor: "var(--accent)",
                    width: 14,
                    height: 14,
                    cursor: "pointer",
                  }}
                />
                <span
                  className="mono"
                  style={{
                    fontSize: 11,
                    width: 13,
                    textAlign: "center",
                    color: STATUS_COLOR[f.status],
                    fontWeight: 700,
                  }}
                >
                  {f.status}
                </span>
                <span
                  className="mono"
                  style={{
                    fontSize: 11,
                    flex: 1,
                    whiteSpace: "nowrap",
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    color: "var(--txt)",
                  }}
                >
                  {f.name}
                </span>
                <span
                  className="mono"
                  style={{ fontSize: 9.5, color: "var(--done)" }}
                >
                  +{f.added}
                </span>
                <span
                  className="mono"
                  style={{ fontSize: 9.5, color: "var(--error)" }}
                >
                  −{f.removed}
                </span>
              </div>
            );
          })
        )}
      </div>

      {/* Diff view + commit bar */}
      <div
        style={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
        }}
      >
        {/* Diff view */}
        <div style={{ flex: 1, overflow: "auto" }}>
          {activeFile ? (
            <DiffView
              rows={activeFile.diffRows}
              filename={activeFile.name}
            />
          ) : (
            <div
              style={{
                padding: 24,
                color: "var(--txt3)",
                fontSize: 12,
              }}
            >
              Select a file to view its diff.
            </div>
          )}
        </div>

        {/* Commit bar */}
        <div
          style={{
            flexShrink: 0,
            borderTop: "1px solid var(--border)",
            padding: "12px 16px",
            background: "var(--panel)",
            display: "flex",
            alignItems: "center",
            gap: 11,
          }}
        >
          <span style={{ fontSize: 12, color: "var(--txt2)" }}>
            {stagedCount} staged
          </span>
          <span style={{ flex: 1 }} />
          <button
            style={{
              fontFamily: "inherit",
              fontWeight: 600,
              fontSize: 12,
              padding: "8px 13px",
              borderRadius: 8,
              border: "1px solid var(--border2)",
              background: "var(--panel2)",
              color: "var(--txt2)",
              cursor: "pointer",
            }}
          >
            Discard
          </button>
          <input
            value={commitMsg}
            onChange={(e) => setCommitMsg(e.target.value)}
            placeholder="Commit message…"
            style={{
              width: 260,
              background: "var(--panel2)",
              border: "1px solid var(--border2)",
              borderRadius: 8,
              padding: "8px 11px",
              color: "var(--txt)",
              fontFamily: "inherit",
              fontSize: 12,
              outline: "none",
            }}
          />
          <button
            onClick={handleCommit}
            style={{
              fontFamily: "inherit",
              fontWeight: 700,
              fontSize: 12,
              padding: "8px 16px",
              borderRadius: 8,
              border: "none",
              background: "var(--done)",
              color: "#0e1116",
              cursor: "pointer",
            }}
          >
            Commit ✓
          </button>
        </div>
      </div>
    </div>
  );
}
