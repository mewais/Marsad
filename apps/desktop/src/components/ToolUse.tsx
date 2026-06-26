/**
 * ToolUse — collapsible tool-call row for the Activity transcript.
 *
 * Collapsed (default): one line — ⚙ icon + tool name + short human-readable
 * summary of the most important argument.
 *
 * Expanded: each argument on its own labelled row; multi-line strings get a
 * <pre> block; nested objects/arrays are pretty-printed with 2-space indent.
 */
import { useState } from "react";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Parse the raw args string (which is a JSON string from the backend). */
function parseArgs(raw?: string): Record<string, unknown> | null {
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      return parsed as Record<string, unknown>;
    }
    return null;
  } catch {
    return null;
  }
}

/** Extract just the filename from a path (or return the full path if short). */
function basename(p: string): string {
  const parts = p.replace(/\\/g, "/").split("/");
  return parts[parts.length - 1] || p;
}

/** Truncate a string to max chars, adding ellipsis if needed. */
function trunc(s: string, max = 72): string {
  const clean = s.replace(/\s+/g, " ").trim();
  return clean.length > max ? clean.slice(0, max) + "…" : clean;
}

/**
 * Build a short human-readable summary for the collapsed view.
 * Returns a plain string (no JSX — it goes into a <span>).
 */
function collapsedSummary(tool: string, args: Record<string, unknown> | null, rawArgs?: string): string {
  if (!args) {
    // Not valid JSON — show raw, truncated
    return rawArgs ? trunc(rawArgs) : "";
  }

  const name = tool.toLowerCase();

  // Bash / shell execution tools
  if (name === "bash" || name === "shell" || name === "execute") {
    const cmd = args["command"] ?? args["cmd"];
    if (typeof cmd === "string") return trunc(cmd);
  }

  // File-oriented tools
  if (["read", "edit", "write", "notebookedit", "multiedit"].includes(name)) {
    const fp = args["file_path"] ?? args["path"] ?? args["notebook_path"];
    if (typeof fp === "string") return basename(fp);
  }

  // Search / grep tools
  if (name === "grep" || name === "grepsearch") {
    const pattern = args["pattern"] ?? args["query"] ?? args["regex"];
    const path = args["path"] ?? args["include"];
    if (typeof pattern === "string") {
      return path ? `${trunc(pattern, 40)} in ${trunc(String(path), 30)}` : trunc(pattern);
    }
  }

  // Glob / find tools
  if (name === "glob" || name === "globsearch" || name === "find") {
    const pattern = args["pattern"] ?? args["glob"];
    const path = args["path"];
    if (typeof pattern === "string") {
      return path ? `${trunc(pattern, 40)} in ${trunc(String(path), 30)}` : trunc(pattern);
    }
  }

  // Task / agent spawn tools
  if (name === "task" || name === "agent" || name === "subtask") {
    const desc = args["description"] ?? args["subagent_type"] ?? args["type"];
    if (typeof desc === "string") return trunc(desc);
  }

  // Web tools
  if (name === "webfetch" || name === "fetch") {
    const url = args["url"];
    if (typeof url === "string") return trunc(url);
  }
  if (name === "websearch" || name === "search") {
    const q = args["query"] ?? args["q"];
    if (typeof q === "string") return trunc(q);
  }

  // TodoWrite / TodoRead
  if (name === "todowrite" || name === "todoread") {
    const todos = args["todos"];
    if (Array.isArray(todos)) return `${todos.length} item${todos.length === 1 ? "" : "s"}`;
  }

  // Generic fallback: first few top-level string/number fields as key=value
  const pairs: string[] = [];
  for (const [k, v] of Object.entries(args)) {
    if (typeof v === "string" || typeof v === "number" || typeof v === "boolean") {
      pairs.push(`${k}=${trunc(String(v), 28)}`);
      if (pairs.length >= 3) break;
    }
  }
  return pairs.length > 0 ? pairs.join(", ") : "";
}

// ---------------------------------------------------------------------------
// Expanded detail renderer
// ---------------------------------------------------------------------------

function ArgValue({ value }: { value: unknown }) {
  if (value === null || value === undefined) {
    return (
      <span
        className="mono"
        style={{ color: "var(--txt3)", fontSize: "calc(13px * var(--fs,1))" }}
      >
        null
      </span>
    );
  }

  if (typeof value === "boolean") {
    return (
      <span
        className="mono"
        style={{ color: "var(--accent2)", fontSize: "calc(13px * var(--fs,1))" }}
      >
        {String(value)}
      </span>
    );
  }

  if (typeof value === "number") {
    return (
      <span
        className="mono"
        style={{ color: "var(--accent2)", fontSize: "calc(13px * var(--fs,1))" }}
      >
        {String(value)}
      </span>
    );
  }

  if (typeof value === "string") {
    const isMultiline = value.includes("\n");
    if (isMultiline || value.length > 120) {
      return (
        <pre
          className="mono"
          style={{
            margin: "4px 0 0",
            padding: "8px 11px",
            borderRadius: 7,
            background: "var(--panel2)",
            border: "1px solid var(--border)",
            fontSize: "calc(12.5px * var(--fs,1))",
            lineHeight: 1.55,
            color: "var(--txt2)",
            whiteSpace: "pre-wrap",
            overflowX: "auto",
            maxHeight: 320,
            overflowY: "auto",
            wordBreak: "break-word",
          }}
        >
          {value}
        </pre>
      );
    }
    return (
      <span
        className="mono"
        style={{
          color: "var(--txt2)",
          fontSize: "calc(13px * var(--fs,1))",
          wordBreak: "break-word",
        }}
      >
        {value}
      </span>
    );
  }

  // Object or array — pretty-print
  const pretty = JSON.stringify(value, null, 2);
  return (
    <pre
      className="mono"
      style={{
        margin: "4px 0 0",
        padding: "8px 11px",
        borderRadius: 7,
        background: "var(--panel2)",
        border: "1px solid var(--border)",
        fontSize: "calc(12.5px * var(--fs,1))",
        lineHeight: 1.55,
        color: "var(--txt2)",
        whiteSpace: "pre-wrap",
        overflowX: "auto",
        maxHeight: 320,
        overflowY: "auto",
      }}
    >
      {pretty}
    </pre>
  );
}

function ExpandedArgs({ args, rawArgs }: { args: Record<string, unknown> | null; rawArgs?: string }) {
  if (!args) {
    // Not valid JSON — show raw text
    return (
      <pre
        className="mono"
        style={{
          margin: "10px 0 0",
          padding: "8px 11px",
          borderRadius: 7,
          background: "var(--panel2)",
          border: "1px solid var(--border)",
          fontSize: "calc(12.5px * var(--fs,1))",
          lineHeight: 1.55,
          color: "var(--txt2)",
          whiteSpace: "pre-wrap",
          overflowX: "auto",
          maxHeight: 320,
          overflowY: "auto",
        }}
      >
        {rawArgs ?? ""}
      </pre>
    );
  }

  const entries = Object.entries(args);
  if (entries.length === 0) {
    return (
      <span
        style={{
          display: "block",
          marginTop: 8,
          color: "var(--txt3)",
          fontSize: "calc(12.5px * var(--fs,1))",
        }}
      >
        (no arguments)
      </span>
    );
  }

  return (
    <div style={{ marginTop: 10, display: "flex", flexDirection: "column", gap: 8 }}>
      {entries.map(([key, val]) => (
        <div key={key}>
          <span
            className="mono"
            style={{
              fontSize: "calc(12px * var(--fs,1))",
              fontWeight: 600,
              color: "var(--accent2)",
              marginRight: 6,
            }}
          >
            {key}
          </span>
          <ArgValue value={val} />
        </div>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Public component
// ---------------------------------------------------------------------------

interface ToolUseProps {
  tool: string;
  args?: string;
  toolStatus?: string;
}

export function ToolUse({ tool, args, toolStatus }: ToolUseProps) {
  const [expanded, setExpanded] = useState(false);
  const parsed = parseArgs(args);
  const summary = collapsedSummary(tool, parsed, args);

  return (
    <div
      style={{
        marginLeft: 37,
        borderRadius: 9,
        background: "var(--panel)",
        border: "1px solid var(--border)",
        fontSize: "calc(14px * var(--fs,1))",
        overflow: "hidden",
      }}
    >
      {/* Collapsed header row — always visible, clickable */}
      <button
        onClick={() => setExpanded((x) => !x)}
        aria-expanded={expanded}
        style={{
          all: "unset",
          display: "flex",
          alignItems: "center",
          gap: 9,
          width: "100%",
          padding: "9px 12px",
          cursor: "pointer",
          boxSizing: "border-box",
        }}
      >
        {/* Chevron */}
        <span
          style={{
            fontSize: "calc(11px * var(--fs,1))",
            color: "var(--txt3)",
            transition: "transform 0.15s",
            transform: expanded ? "rotate(90deg)" : "rotate(0deg)",
            display: "inline-block",
            width: 11,
            flexShrink: 0,
          }}
        >
          ▶
        </span>

        {/* Gear icon */}
        <span
          style={{
            fontSize: "calc(13.5px * var(--fs,1))",
            color: "var(--accent2)",
            flexShrink: 0,
          }}
        >
          ⚙
        </span>

        {/* Tool name */}
        <span
          style={{
            fontWeight: 700,
            color: "var(--txt)",
            flexShrink: 0,
          }}
        >
          {tool}
        </span>

        {/* Summary */}
        {summary && (
          <span
            className="mono"
            style={{
              color: "var(--txt2)",
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
              flex: 1,
              fontSize: "calc(13px * var(--fs,1))",
            }}
          >
            {summary}
          </span>
        )}

        {/* Spacer when no summary to push status to the right */}
        {!summary && <span style={{ flex: 1 }} />}

        {/* Status badge */}
        {toolStatus && (
          <span
            style={{
              fontSize: "calc(12.5px * var(--fs,1))",
              color: "var(--done)",
              flexShrink: 0,
            }}
          >
            {toolStatus}
          </span>
        )}
      </button>

      {/* Expanded detail panel */}
      {expanded && (
        <div
          style={{
            padding: "2px 12px 12px 32px",
            borderTop: "1px solid var(--border)",
          }}
        >
          <ExpandedArgs args={parsed} rawArgs={args} />
        </div>
      )}
    </div>
  );
}
