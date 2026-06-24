import type { SideRow } from "../bridge/types";

interface DiffViewProps {
  rows: SideRow[];
  filename?: string;
}

/** Side-by-side diff view faithful to the .dc.html design. */
export function DiffView({ rows, filename }: DiffViewProps) {
  return (
    <div style={{ display: "flex", flexDirection: "column", overflow: "hidden" }}>
      {/* Header bar */}
      <div
        className="mono"
        style={{
          fontSize: "calc(12.5px * var(--fs,1))",
          padding: "9px 16px",
          color: "var(--txt2)",
          borderBottom: "1px solid var(--border)",
          background: "var(--panel)",
          display: "flex",
          alignItems: "center",
          gap: 10,
        }}
      >
        <span style={{ flex: 1 }}>{filename ?? ""}</span>
        <span
          style={{
            fontSize: "calc(11.5px * var(--fs,1))",
            letterSpacing: 0.5,
            textTransform: "uppercase",
            color: "var(--txt3)",
          }}
        >
          Side-by-side
        </span>
      </div>

      {/* Column labels */}
      <div
        style={{
          display: "flex",
          background: "var(--panel)",
          borderBottom: "1px solid var(--border)",
          fontSize: "calc(11.5px * var(--fs,1))",
          fontWeight: 700,
          letterSpacing: 0.5,
          textTransform: "uppercase",
          color: "var(--txt3)",
          position: "sticky",
          top: 0,
          zIndex: 1,
        }}
      >
        <span style={{ flex: 1, padding: "7px 14px" }}>Before</span>
        <span
          style={{
            flex: 1,
            padding: "7px 14px",
            borderLeft: "1px solid var(--border2)",
          }}
        >
          After
        </span>
      </div>

      {/* Rows */}
      {rows.map((row, i) => (
        <div
          key={i}
          style={{
            display: "flex",
            fontFamily: "var(--font-mono, 'JetBrains Mono', monospace)",
            fontSize: "calc(14px * var(--fs,1))",
            lineHeight: 1.7,
          }}
        >
          {/* Left cell */}
          <div
            style={{ flex: 1, display: "flex", minWidth: 0, background: row.l.bg }}
          >
            <span
              style={{
                width: 38,
                flexShrink: 0,
                textAlign: "right",
                paddingRight: 9,
                color: "var(--txt3)",
                userSelect: "none",
                opacity: 0.55,
              }}
            >
              {row.l.ln}
            </span>
            <span
              style={{
                width: 14,
                flexShrink: 0,
                textAlign: "center",
                color: row.l.sigColor,
              }}
            >
              {row.l.sig}
            </span>
            <span
              style={{
                whiteSpace: "pre-wrap",
                color: row.l.fg,
                paddingRight: 10,
                flex: 1,
                minWidth: 0,
              }}
            >
              {row.l.text}
            </span>
          </div>

          {/* Right cell */}
          <div
            style={{
              flex: 1,
              display: "flex",
              minWidth: 0,
              background: row.r.bg,
              borderLeft: "1px solid var(--border2)",
            }}
          >
            <span
              style={{
                width: 38,
                flexShrink: 0,
                textAlign: "right",
                paddingRight: 9,
                color: "var(--txt3)",
                userSelect: "none",
                opacity: 0.55,
              }}
            >
              {row.r.ln}
            </span>
            <span
              style={{
                width: 14,
                flexShrink: 0,
                textAlign: "center",
                color: row.r.sigColor,
              }}
            >
              {row.r.sig}
            </span>
            <span
              style={{
                whiteSpace: "pre-wrap",
                color: row.r.fg,
                paddingRight: 10,
                flex: 1,
                minWidth: 0,
              }}
            >
              {row.r.text}
            </span>
          </div>
        </div>
      ))}
    </div>
  );
}
