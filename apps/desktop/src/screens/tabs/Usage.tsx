/**
 * Usage tab — context window bar (amber warning ≥75%), token/cost/elapsed tiles,
 * and a per-model breakdown table when modelUsage data is available.
 * All values sourced from selectedDetail (SessionDetailVM) and selectedSession.
 */
import { useVm } from "../../bridge/useVm";

export function UsageTab() {
  const { selectedDetail, selectedSession } = useVm();

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
          maxWidth: 900,
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
              fontSize: "calc(15px * var(--fs,1))",
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
            <span style={{ fontSize: "calc(14.5px * var(--fs,1))", fontWeight: 700 }}>
              Context window
            </span>
            <span
              className="mono"
              style={{ fontSize: "calc(14.5px * var(--fs,1))", color: "var(--txt2)" }}
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
                fontSize: "calc(13.5px * var(--fs,1))",
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
              className="mono"
              style={{ fontSize: "calc(29px * var(--fs,1))", fontWeight: 800, marginTop: 5 }}
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
                fontSize: "calc(13.5px * var(--fs,1))",
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
              className="mono"
              style={{ fontSize: "calc(29px * var(--fs,1))", fontWeight: 800, marginTop: 5 }}
            >
              {selectedSession?.cost_estimated && (
                <span
                  style={{
                    fontSize: "0.55em",
                    fontWeight: 700,
                    color: "var(--txt3)",
                    marginRight: 4,
                    verticalAlign: "middle",
                  }}
                  title="Estimated from the local pricing table — no agent-reported cost has been received yet"
                >
                  ~ estimated
                </span>
              )}
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
                fontSize: "calc(13.5px * var(--fs,1))",
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
              className="mono"
              style={{ fontSize: "calc(29px * var(--fs,1))", fontWeight: 800, marginTop: 5 }}
            >
              {elapsed_label || "—"}
            </div>
          </div>
        </div>

        {/* Per-model breakdown — only shown when modelUsage data is available */}
        {selectedSession && selectedSession.model_usage.length > 0 && (
          <div
            data-testid="model-usage-table"
            style={{
              padding: 18,
              borderRadius: 13,
              border: "1px solid var(--border)",
              background: "var(--panel)",
            }}
          >
            <div
              style={{
                fontSize: "calc(13.5px * var(--fs,1))",
                color: "var(--txt3)",
                textTransform: "uppercase" as const,
                letterSpacing: 0.4,
                fontWeight: 700,
                marginBottom: 12,
              }}
            >
              Model Breakdown
            </div>
            <table
              style={{
                width: "100%",
                borderCollapse: "collapse",
                fontSize: "calc(13.5px * var(--fs,1))",
              }}
            >
              <thead>
                <tr
                  style={{
                    color: "var(--txt3)",
                    fontWeight: 700,
                    textAlign: "left",
                    borderBottom: "1px solid var(--border)",
                  }}
                >
                  <th style={{ paddingBottom: 8, paddingRight: 16 }}>Model</th>
                  <th style={{ paddingBottom: 8, paddingRight: 16 }}>Input Tokens</th>
                  <th style={{ paddingBottom: 8, paddingRight: 16 }}>Output Tokens</th>
                  <th style={{ paddingBottom: 8 }}>Cost (USD)</th>
                </tr>
              </thead>
              <tbody>
                {selectedSession.model_usage.map((row) => (
                  <tr
                    key={row.model}
                    style={{ borderBottom: "1px solid var(--border)" }}
                    data-testid={`model-usage-row-${row.model}`}
                  >
                    <td
                      className="mono"
                      style={{ paddingTop: 8, paddingBottom: 8, paddingRight: 16, color: "var(--accent)" }}
                    >
                      {row.model}
                    </td>
                    <td
                      className="mono"
                      style={{ paddingTop: 8, paddingBottom: 8, paddingRight: 16 }}
                    >
                      {row.input_tokens.toLocaleString()}
                    </td>
                    <td
                      className="mono"
                      style={{ paddingTop: 8, paddingBottom: 8, paddingRight: 16 }}
                    >
                      {row.output_tokens.toLocaleString()}
                    </td>
                    <td
                      className="mono"
                      style={{ paddingTop: 8, paddingBottom: 8 }}
                    >
                      ${row.cost_usd.toFixed(4)}
                      {row.estimated && (
                        <span
                          style={{
                            marginLeft: 6,
                            fontStyle: "italic",
                            fontSize: "0.85em",
                            color: "var(--txt3)",
                          }}
                          title="Estimated from the local pricing table — not agent-reported"
                        >
                          ~ estimated
                        </span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </div>
  );
}
