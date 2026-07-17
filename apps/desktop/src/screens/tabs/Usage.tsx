/**
 * Usage tab — context window bar (amber warning ≥75%), token/cost/elapsed tiles,
 * a per-model breakdown table when modelUsage data is available, and the
 * workstream-scoped Cost breakdown (By host / By node — Wave 3a,
 * docs/design/DESIGN_HANDOFF.md §"Usage tab").
 * All values sourced from selectedDetail (SessionDetailVM) and selectedSession;
 * the Cost breakdown is fetched separately, keyed on the open workstream id.
 */
import { useEffect, useState } from "react";
import { useVm } from "../../bridge/useVm";
import { fetchCostBreakdown } from "../../bridge/api";
import type { CostRow, WorkstreamCostBreakdown } from "../../bridge/types";
import type { Driver } from "../../bridge/workstreamTypes";

// ---------------------------------------------------------------------------
// Driver pill colours — mirrors Board.tsx / graph/GraphScreen.tsx /
// SessionDetail.tsx's `DRIVER_COLOR` map (kept local here rather than shared
// since those files belong to other redesign slices this one must not touch).
// ---------------------------------------------------------------------------

const DRIVER_COLOR: Record<Driver, string> = {
  claude: "#d97757",
  codex: "#3fb6a8",
  gemini: "#9b8cf0",
  opencode: "#f0a050",
};

/** driver hex -> ~12-14% tint background, matching the design tokens' "driver
 *  pills use the hex as text + border, over a tint as background". */
function driverTint(hex: string): string {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return `rgba(${r},${g},${b},0.14)`;
}

// ---------------------------------------------------------------------------
// Cost breakdown — stacked bar + per-row entries, shared by "By host"/"By node"
// ---------------------------------------------------------------------------

function CostBarSegments({ rows }: { rows: CostRow[] }) {
  return (
    <div
      style={{
        display: "flex",
        height: 10,
        borderRadius: 5,
        overflow: "hidden",
        background: "var(--border)",
      }}
    >
      {rows
        .filter((r) => r.pct > 0)
        .map((r) => (
          <div
            key={r.key}
            title={`${r.label} — $${r.cost_usd.toFixed(2)} (${r.pct.toFixed(0)}%)`}
            style={{
              width: `${Math.max(0, Math.min(100, r.pct))}%`,
              background: DRIVER_COLOR[r.driver_dots[0] ?? "claude"],
            }}
          />
        ))}
    </div>
  );
}

function CostRowEntry({ row }: { row: CostRow }) {
  return (
    <div
      data-testid={`cost-row-${row.key}`}
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        gap: 10,
        padding: "7px 0",
        borderBottom: "1px solid var(--border)",
        fontSize: "calc(13px * var(--fs,1))",
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 8, minWidth: 0 }}>
        <span style={{ display: "flex", gap: 4, flexShrink: 0 }}>
          {row.driver_dots.map((d, i) => (
            <span
              key={`${row.key}-${d}-${i}`}
              data-testid="cost-driver-pill"
              data-driver={d}
              title={d}
              style={{
                fontSize: "calc(10.5px * var(--fs,1))",
                fontWeight: 700,
                padding: "1px 6px",
                borderRadius: 5,
                color: DRIVER_COLOR[d],
                background: driverTint(DRIVER_COLOR[d]),
                border: `1px solid ${DRIVER_COLOR[d]}`,
                whiteSpace: "nowrap",
              }}
            >
              {d}
            </span>
          ))}
        </span>
        <span
          className="mono"
          style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
        >
          {row.label}
        </span>
      </div>
      <div className="mono" style={{ display: "flex", alignItems: "center", gap: 8, flexShrink: 0 }}>
        {row.estimated && (
          <span
            style={{
              fontSize: "0.85em",
              fontStyle: "italic",
              color: "var(--txt3)",
            }}
            title="Estimated from the local pricing table — no agent-reported cost has been received yet"
          >
            ~ estimated
          </span>
        )}
        <span>${row.cost_usd.toFixed(2)}</span>
        <span style={{ color: "var(--txt3)", minWidth: 38, textAlign: "right" }}>
          {row.pct.toFixed(0)}%
        </span>
      </div>
    </div>
  );
}

function CostGroup({ title, rows, testId }: { title: string; rows: CostRow[]; testId: string }) {
  if (rows.length === 0) return null;
  return (
    <div data-testid={testId} style={{ marginTop: 14 }}>
      <div
        style={{
          fontSize: "calc(12px * var(--fs,1))",
          color: "var(--txt3)",
          textTransform: "uppercase" as const,
          letterSpacing: 0.4,
          fontWeight: 700,
          marginBottom: 8,
        }}
      >
        {title}
      </div>
      <CostBarSegments rows={rows} />
      <div style={{ marginTop: 6 }}>
        {rows.map((row) => (
          <CostRowEntry key={row.key} row={row} />
        ))}
      </div>
    </div>
  );
}

function CostBreakdownSection({ breakdown }: { breakdown: WorkstreamCostBreakdown }) {
  if (breakdown.by_host.length === 0 && breakdown.by_node.length === 0) return null;
  return (
    <div
      data-testid="cost-breakdown"
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
          alignItems: "baseline",
        }}
      >
        <span
          style={{
            fontSize: "calc(13.5px * var(--fs,1))",
            color: "var(--txt3)",
            textTransform: "uppercase" as const,
            letterSpacing: 0.4,
            fontWeight: 700,
          }}
        >
          Cost breakdown
        </span>
        <span data-testid="cost-breakdown-total" className="mono" style={{ fontWeight: 700 }}>
          {breakdown.total_estimated && (
            <span
              style={{
                fontSize: "0.75em",
                fontWeight: 700,
                color: "var(--txt3)",
                marginRight: 4,
              }}
              title="Estimated from the local pricing table — no agent-reported cost has been received yet"
            >
              ~ estimated
            </span>
          )}
          ${breakdown.total_cost_usd.toFixed(2)}
        </span>
      </div>
      <CostGroup title="By host" rows={breakdown.by_host} testId="cost-by-host" />
      <CostGroup title="By node" rows={breakdown.by_node} testId="cost-by-node" />
    </div>
  );
}

// ---------------------------------------------------------------------------
// UsageTab
// ---------------------------------------------------------------------------

export function UsageTab() {
  const { selectedDetail, selectedSession, state } = useVm();
  const wsId = state.wsId;

  // Fetched separately from selectedDetail/selectedSession — the cost
  // breakdown is workstream-scoped (Core::cost_breakdown(ws_id)), not
  // per-session, so it's kept as local async state keyed on the open
  // workstream rather than folded into useVm's synchronous selectors.
  const [breakdown, setBreakdown] = useState<WorkstreamCostBreakdown | null>(null);

  useEffect(() => {
    let cancelled = false;
    if (!wsId) {
      setBreakdown(null);
      return;
    }
    fetchCostBreakdown(wsId).then((bd) => {
      if (!cancelled) setBreakdown(bd);
    });
    return () => {
      cancelled = true;
    };
  }, [wsId]);

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

        {/* Cost breakdown — By host / By node (Wave 3a) */}
        {breakdown && <CostBreakdownSection breakdown={breakdown} />}
      </div>
    </div>
  );
}
