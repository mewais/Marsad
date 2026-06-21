/**
 * Usage tab — context window bar (amber warning ≥75%), token/cost/elapsed tiles.
 * All values sourced from selectedDetail (SessionDetailVM).
 */
import { useVm } from "../../bridge/useVm";

export function UsageTab() {
  const { selectedDetail } = useVm();

  if (!selectedDetail) {
    return (
      <div style={{ padding: 20, color: "var(--txt3)" }}>
        No session selected.
      </div>
    );
  }

  const {
    ctx_warn,
    context_pct,
    tokens_label,
    cost_label,
    elapsed_label,
  } = selectedDetail;

  // Context bar color: amber when ≥75%, accent otherwise
  const ctxColor = ctx_warn ? "var(--needs)" : "var(--accent)";
  const ctxWidth = `${Math.min(100, Math.max(0, context_pct))}%`;

  return (
    <div style={{ flex: 1, overflowY: "auto", padding: 24 }}>
      <div
        style={{
          maxWidth: 660,
          margin: "0 auto",
          display: "flex",
          flexDirection: "column",
          gap: 16,
        }}
      >
        {/* Context warning banner */}
        {ctx_warn && (
          <div
            data-testid="ctx-warn-banner"
            style={{
              display: "flex",
              alignItems: "center",
              gap: 10,
              padding: "13px 15px",
              borderRadius: 11,
              border: "1px solid var(--needs)",
              background: "var(--needs-bg)",
              color: "var(--needs)",
              fontSize: 12.5,
              fontWeight: 600,
            }}
          >
            ⚠ Context window {context_pct}% full — auto-compaction is
            approaching. Consider wrapping up or starting a fresh session to
            avoid degraded quality.
          </div>
        )}

        {/* Context bar */}
        <div
          style={{
            padding: 18,
            borderRadius: 13,
            border: "1px solid var(--border)",
            background: "var(--panel)",
          }}
        >
          <div
            style={{
              display: "flex",
              justifyContent: "space-between",
              marginBottom: 8,
            }}
          >
            <span style={{ fontSize: 12, fontWeight: 700 }}>
              Context window
            </span>
            <span
              className="mono"
              style={{ fontSize: 12, color: "var(--txt2)" }}
              data-testid="ctx-pct-label"
            >
              {context_pct}% used
            </span>
          </div>
          <div
            style={{
              height: 9,
              borderRadius: 5,
              background: "var(--border)",
              overflow: "hidden",
            }}
          >
            <div
              data-testid="ctx-bar"
              style={{
                height: "100%",
                borderRadius: 5,
                background: ctxColor,
                width: ctxWidth,
                transition: "width 0.3s ease",
              }}
            />
          </div>
        </div>

        {/* Stat tiles — Tokens / Cost / Elapsed */}
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "1fr 1fr 1fr",
            gap: 12,
          }}
        >
          {/* Tokens */}
          <div
            style={{
              padding: 16,
              borderRadius: 13,
              border: "1px solid var(--border)",
              background: "var(--panel)",
            }}
          >
            <div
              style={{
                fontSize: 11,
                color: "var(--txt3)",
                textTransform: "uppercase" as const,
                letterSpacing: 0.4,
                fontWeight: 700,
              }}
            >
              Tokens
            </div>
            <div
              data-testid="tokens-value"
              style={{ fontSize: 24, fontWeight: 800, marginTop: 5 }}
            >
              {tokens_label}
            </div>
          </div>

          {/* Cost */}
          <div
            style={{
              padding: 16,
              borderRadius: 13,
              border: "1px solid var(--border)",
              background: "var(--panel)",
            }}
          >
            <div
              style={{
                fontSize: 11,
                color: "var(--txt3)",
                textTransform: "uppercase" as const,
                letterSpacing: 0.4,
                fontWeight: 700,
              }}
            >
              Cost
            </div>
            <div
              data-testid="cost-value"
              style={{ fontSize: 24, fontWeight: 800, marginTop: 5 }}
            >
              {cost_label}
            </div>
          </div>

          {/* Elapsed */}
          <div
            style={{
              padding: 16,
              borderRadius: 13,
              border: "1px solid var(--border)",
              background: "var(--panel)",
            }}
          >
            <div
              style={{
                fontSize: 11,
                color: "var(--txt3)",
                textTransform: "uppercase" as const,
                letterSpacing: 0.4,
                fontWeight: 700,
              }}
            >
              Elapsed
            </div>
            <div
              data-testid="elapsed-value"
              style={{ fontSize: 24, fontWeight: 800, marginTop: 5 }}
            >
              {elapsed_label || "—"}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
