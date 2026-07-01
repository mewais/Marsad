/**
 * GraphScreen — Level 2 "Workstream Graph" (By-host layout).
 *
 * Pure/presentational: takes `graph: GraphView`, `selectedNode`, and the
 * `onSelectNode`/`onOpenNode`/`onOpenGate` callbacks as props. No `useVm`,
 * no Tauri, no network — geometry comes from the pure `layoutByHost`.
 *
 * Spec: docs/specs/ui-graph-screen.md §Component + §Test plan GS1-GS8.
 */
import type { Driver, GraphView, Phase } from "../../bridge/workstreamTypes";
import { layoutByHost } from "./layout";
import type { Point } from "./layout";

// ---------------------------------------------------------------------------
// Colour helpers
// ---------------------------------------------------------------------------

const driverColor: Record<Driver, string> = {
  claude: "#d97757",
  codex: "#3fb6a8",
  gemini: "#9b8cf0",
};

/** Node state colour (also drives the left accent + state dot). */
function stateColor(state: Phase): string {
  switch (state) {
    case "Needs":
      return "var(--needs)";
    case "Error":
      return "var(--error)";
    case "Working":
    case "Starting":
      return "var(--accent)";
    case "Done":
      return "var(--done)";
    default:
      return "var(--neutral)";
  }
}

/** A node pulses while it is actively working or needs the user. */
function pulsesFor(state: Phase): boolean {
  return state === "Working" || state === "Needs";
}

/** Edge status colour (normal=accent, consulting=txt2, needs=needs). */
function edgeStatusColor(status: string): string {
  switch (status) {
    case "consulting":
      return "var(--txt2)";
    case "needs":
      return "var(--needs)";
    default:
      return "var(--accent)";
  }
}

/** Build an SVG polyline path string from routed edge points. */
function pointsToPath(points: Point[]): string {
  if (points.length === 0) return "";
  const [first, ...rest] = points;
  return (
    `M ${first.x} ${first.y}` +
    rest.map((p) => ` L ${p.x} ${p.y}`).join("")
  );
}

// ---------------------------------------------------------------------------
// GraphScreen
// ---------------------------------------------------------------------------

export function GraphScreen(props: {
  graph: GraphView;
  selectedNode: string | null;
  onSelectNode: (id: string | null) => void;
  onOpenNode: (id: string) => void;
  onOpenGate: (connId: string) => void;
}): JSX.Element {
  const { graph, selectedNode, onSelectNode, onOpenNode, onOpenGate } = props;
  const layout = layoutByHost(graph);

  // Neighbourhood of the selected node: itself + any node connected by an edge.
  const neighbourhood = new Set<string>();
  if (selectedNode) {
    neighbourhood.add(selectedNode);
    for (const e of graph.edges) {
      if (e.from === selectedNode) neighbourhood.add(e.to);
      if (e.to === selectedNode) neighbourhood.add(e.from);
    }
  }

  const canvasContentW = layout.width;
  const canvasContentH = layout.height;

  return (
    <div
      style={{
        display: "flex",
        gap: 16,
        padding: "22px 26px 40px",
        color: "var(--txt)",
        alignItems: "flex-start",
      }}
    >
      {/* Graph column: lane headers above the positioned canvas ------------ */}
      <div style={{ flex: "0 0 auto" }}>
        {/* Host swimlane headers */}
        <div style={{ position: "relative", width: canvasContentW, height: 34 }}>
          {layout.hosts.map((host, col) => {
            const laneX = col * (222 + 30);
            return (
              <div
                key={host}
                data-testid={`host-lane-${host}`}
                className="mono"
                style={{
                  position: "absolute",
                  left: laneX,
                  top: 0,
                  width: 222,
                  height: 34,
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  fontSize: "calc(11.5px * var(--fs,1))",
                  fontWeight: 700,
                  letterSpacing: 0.4,
                  color: "var(--txt3)",
                  textTransform: "uppercase",
                }}
              >
                ▪ {host}
              </div>
            );
          })}
        </div>

        {/* Positioned canvas */}
        <div
          data-testid="graph-canvas"
          style={{
            position: "relative",
            width: canvasContentW,
            height: canvasContentH,
          }}
        >
        {/* Edge layer (below nodes) */}
        <svg
          width={canvasContentW}
          height={canvasContentH}
          style={{
            position: "absolute",
            left: 0,
            top: 0,
            pointerEvents: "none",
            overflow: "visible",
          }}
        >
          {layout.edges.map((edge) => {
            const graphEdge = graph.edges.find((e) => e.id === edge.id);
            const kind = graphEdge?.kind ?? "data";
            const color = edgeStatusColor(edge.status);
            const dash = kind === "command" ? "6 5" : undefined;
            return (
              <g key={edge.id}>
                <path
                  data-testid={`graph-edge-${edge.id}`}
                  data-status={edge.status}
                  data-kind={kind}
                  d={pointsToPath(edge.points)}
                  fill="none"
                  stroke={color}
                  strokeWidth={2}
                  strokeDasharray={dash}
                />
                {edge.gate && (
                  <rect
                    data-testid={`gate-diamond-${edge.id}`}
                    x={edge.gate.x - 8}
                    y={edge.gate.y - 8}
                    width={16}
                    height={16}
                    transform={`rotate(45 ${edge.gate.x} ${edge.gate.y})`}
                    fill="var(--needs)"
                    stroke="var(--panel)"
                    strokeWidth={1.5}
                    style={{ cursor: "pointer", pointerEvents: "all" }}
                    onClick={() => onOpenGate(edge.id)}
                  />
                )}
              </g>
            );
          })}
        </svg>

        {/* Node layer */}
        {layout.nodes.map((positioned) => {
          const n = positioned.node;
          const isSelected = n.id === selectedNode;
          const isDimmed = selectedNode != null && !neighbourhood.has(n.id);
          const color = stateColor(n.state);
          const pulse = pulsesFor(n.state);
          return (
            <div
              key={n.id}
              data-testid={`graph-node-${n.id}`}
              data-driver={n.driver}
              data-state={n.state}
              data-selected={isSelected ? "true" : "false"}
              data-dimmed={isDimmed ? "true" : "false"}
              onClick={() => onSelectNode(n.id)}
              style={{
                position: "absolute",
                left: `${positioned.x}px`,
                top: `${positioned.y}px`,
                width: positioned.w,
                height: positioned.h,
                boxSizing: "border-box",
                background: "var(--panel)",
                border: `1px solid ${isSelected ? color : "var(--border)"}`,
                borderLeft: `3px solid ${color}`,
                borderRadius: 10,
                padding: "8px 10px",
                cursor: "pointer",
                opacity: isDimmed ? 0.32 : 1,
                boxShadow: isSelected ? `0 0 18px var(--accent-bg)` : "var(--shadow-sm)",
                overflow: "hidden",
                display: "flex",
                flexDirection: "column",
                gap: 4,
              }}
            >
              {/* Header row: state dot + name + open affordance */}
              <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                <span
                  data-pulse={pulse ? "true" : "false"}
                  style={{
                    width: 8,
                    height: 8,
                    borderRadius: "50%",
                    background: color,
                    display: "inline-block",
                    flexShrink: 0,
                    animation: pulse ? "dotpulse 1.4s infinite" : undefined,
                  }}
                />
                <span
                  style={{
                    fontWeight: 700,
                    fontSize: "calc(13px * var(--fs,1))",
                    whiteSpace: "nowrap",
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    flex: 1,
                  }}
                >
                  {n.name}
                </span>
                <span
                  data-testid={`node-open-${n.id}`}
                  role="button"
                  title="Open session"
                  onClick={(ev) => {
                    ev.stopPropagation();
                    onOpenNode(n.id);
                  }}
                  style={{ color: "var(--txt3)", cursor: "pointer", flexShrink: 0 }}
                >
                  ›
                </span>
              </div>

              {/* Meta row: driver pill + status text */}
              <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                <span
                  data-testid="driver-pill"
                  data-driver={n.driver}
                  style={{
                    fontSize: "calc(10.5px * var(--fs,1))",
                    fontWeight: 700,
                    padding: "1px 6px",
                    borderRadius: 5,
                    color: "#ffffff",
                    background: driverColor[n.driver],
                    flexShrink: 0,
                  }}
                >
                  {n.driver}
                </span>
                <span
                  style={{
                    fontSize: "calc(11px * var(--fs,1))",
                    color: "var(--txt3)",
                    whiteSpace: "nowrap",
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                  }}
                >
                  {n.status}
                </span>
              </div>
            </div>
          );
        })}
        </div>
      </div>

      {/* Inspector -------------------------------------------------------- */}
      <div
        data-testid="graph-inspector"
        style={{
          flex: "0 0 260px",
          width: 260,
          background: "var(--panel)",
          border: "1px solid var(--border)",
          borderRadius: 13,
          padding: 16,
          display: "flex",
          flexDirection: "column",
          gap: 12,
        }}
      >
        <div>
          <div
            style={{
              fontWeight: 700,
              fontSize: "calc(15.5px * var(--fs,1))",
              letterSpacing: -0.2,
            }}
          >
            {graph.name}
          </div>
          <div
            style={{
              fontSize: "calc(11.5px * var(--fs,1))",
              color: "var(--txt3)",
              marginTop: 2,
            }}
          >
            {graph.goal}
          </div>
        </div>

        {/* Counts */}
        <div style={{ display: "flex", gap: 8 }}>
          <span
            data-testid="inspector-working"
            style={{
              fontSize: "calc(11.5px * var(--fs,1))",
              fontWeight: 700,
              padding: "3px 8px",
              borderRadius: 6,
              color: "var(--accent)",
              background: "var(--working-bg)",
            }}
          >
            {graph.inspector.working} working
          </span>
          <span
            data-testid="inspector-needs"
            style={{
              fontSize: "calc(11.5px * var(--fs,1))",
              fontWeight: 700,
              padding: "3px 8px",
              borderRadius: 6,
              color: "var(--needs)",
              background: "var(--needs-bg)",
            }}
          >
            {graph.inspector.needs} needs
          </span>
        </div>

        {/* Pending handoff callout */}
        {graph.inspector.pending_handoff && (
          <div
            data-testid="inspector-handoff"
            role="button"
            onClick={() => onOpenGate(graph.inspector.pending_handoff!.conn_id)}
            style={{
              cursor: "pointer",
              padding: "10px 12px",
              borderRadius: 9,
              background: "var(--needs-bg)",
              border: "1px solid var(--needs)",
              color: "var(--needs)",
              fontSize: "calc(12px * var(--fs,1))",
              fontWeight: 600,
            }}
          >
            <div style={{ fontWeight: 700 }}>Pending handoff</div>
            <div style={{ color: "var(--txt2)", marginTop: 2 }}>
              {graph.inspector.pending_handoff.from} → {graph.inspector.pending_handoff.to}
            </div>
            <div style={{ color: "var(--txt3)", marginTop: 2 }}>
              {graph.inspector.pending_handoff.payload}
            </div>
          </div>
        )}

        {/* Sessions list */}
        <div>
          <div
            style={{
              fontSize: "calc(11px * var(--fs,1))",
              fontWeight: 700,
              letterSpacing: 0.4,
              textTransform: "uppercase",
              color: "var(--txt3)",
              marginBottom: 6,
            }}
          >
            Sessions
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
            {graph.nodes.map((n) => (
              <div
                key={n.id}
                data-testid={`inspector-session-${n.id}`}
                onClick={() => onOpenNode(n.id)}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                  padding: "6px 8px",
                  borderRadius: 7,
                  background: "var(--panel2)",
                  border: "1px solid var(--border)",
                  cursor: "pointer",
                  fontSize: "calc(12px * var(--fs,1))",
                }}
              >
                <span
                  style={{
                    width: 7,
                    height: 7,
                    borderRadius: "50%",
                    background: driverColor[n.driver],
                    display: "inline-block",
                    flexShrink: 0,
                  }}
                />
                <span
                  style={{
                    flex: 1,
                    whiteSpace: "nowrap",
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                  }}
                >
                  {n.name}
                </span>
                <span style={{ color: "var(--txt3)" }}>›</span>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
