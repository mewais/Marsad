/**
 * GraphScreen — Level 2 "Workstream Graph".
 *
 * Mostly-presentational: takes `graph: GraphView`, `selectedNode`, and the
 * `onSelectNode`/`onOpenNode`/`onOpenGate` callbacks as props (no `useVm`, no
 * Tauri, no network — geometry comes from the pure `layoutByHost` /
 * `buildFlowLayout`). Layout-mode/zoom/drag-connect are UI-only concerns kept
 * as internal component state so the required prop surface stays exactly the
 * one the original block shipped (backward compatible with callers that only
 * pass the five base props). Design-mode authoring (drag-to-connect, Confirm
 * & launch / Discard, add-agent) is opt-in via additional OPTIONAL props.
 *
 * Spec: docs/specs/ui-graph-screen.md + docs/design/DESIGN_HANDOFF.md
 * §"LEVEL 2 — Workstream Graph" (canvas, both layouts, edges, gate diamond,
 * toolbar overlays, inspector operate/design).
 */
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, MouseEvent as ReactMouseEvent } from "react";
import type { ConnKind, Driver, GraphView, Phase } from "../../bridge/workstreamTypes";
import {
  buildFlowLayout,
  computeElkLayout,
  layoutByHost,
  smoothEdgePath,
  LANE_W,
  LANE_GAP,
} from "./layout";
import type { GraphLayout, Point } from "./layout";
import { edgeColor, edgeDashStyle, edgeStrokeWidth } from "./edgeStyle";

// ---------------------------------------------------------------------------
// Colour / state helpers
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

const ZOOM_MIN = 0.25;
const ZOOM_MAX = 1.6;
const CANVAS_PAD = 18;

type LayoutMode = "host" | "flow";

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

export interface GraphScreenProps {
  graph: GraphView;
  selectedNode: string | null;
  onSelectNode: (id: string | null) => void;
  onOpenNode: (id: string) => void;
  onOpenGate: (connId: string) => void;

  // ---- Design-mode authoring (all optional; omitted ⇒ pure operate mode) --
  /** True while authoring this workstream's graph (drag-connect handles show,
   *  inspector switches to the Design panel). */
  designMode?: boolean;
  /** "＋ Add agent" above the canvas (design mode) — a stub hook into the
   *  node-add modal, owned by a later slice. */
  onAddAgent?: () => void;
  /** "✎ Update workflow" (operate mode) — re-enters design mode. */
  onEnterDesign?: () => void;
  /** "Confirm & launch" in the Design panel. */
  onConfirmDraft?: () => void;
  /** "Discard" in the Design panel. */
  onDiscardDraft?: () => void;
  /** Drag-to-connect finished + an edge kind was chosen. */
  onConnect?: (from: string, to: string, kind: ConnKind, bi: boolean) => void;

  // ---- Workstream lifecycle (#5, operate-mode inspector) — all optional ----
  /** Pause every session in this workstream. */
  onPauseWorkstream?: () => void;
  /** Resume every session in this workstream. */
  onResumeWorkstream?: () => void;
  /** Delete this workstream (kills all sessions) — confirmed by the caller. */
  onDeleteWorkstream?: () => void;
}

// ---------------------------------------------------------------------------
// GraphScreen
// ---------------------------------------------------------------------------

export function GraphScreen(props: GraphScreenProps): JSX.Element {
  const {
    graph,
    selectedNode,
    onSelectNode,
    onOpenNode,
    onOpenGate,
    designMode = false,
    onAddAgent,
    onEnterDesign,
    onConfirmDraft,
    onDiscardDraft,
    onConnect,
    onPauseWorkstream,
    onResumeWorkstream,
    onDeleteWorkstream,
  } = props;

  const [layoutMode, setLayoutMode] = useState<LayoutMode>("host");
  const [zoom, setZoom] = useState(1);
  const [flowLayout, setFlowLayout] = useState<GraphLayout | null>(null);
  const [drag, setDrag] = useState<{ from: string; x: number; y: number } | null>(null);
  const [edgePick, setEdgePick] = useState<{ from: string; to: string } | null>(null);

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLDivElement | null>(null);

  const hostLayout = useMemo(() => layoutByHost(graph), [graph]);

  // Kick off elkjs whenever Flow mode is active and the graph identity
  // changes. Falls back to rendering nothing new (by-host geometry is used
  // below) if elkjs is unavailable or rejects — never throws into render.
  useEffect(() => {
    if (layoutMode !== "flow") return;
    let cancelled = false;
    setFlowLayout(null);
    computeElkLayout(graph)
      .then((elk) => {
        if (cancelled) return;
        setFlowLayout(buildFlowLayout(graph, elk));
      })
      .catch(() => {
        /* elkjs unavailable/failed — canvas falls back to by-host geometry */
      });
    return () => {
      cancelled = true;
    };
  }, [layoutMode, graph]);

  const layout: GraphLayout = layoutMode === "flow" && flowLayout ? flowLayout : hostLayout;

  // Auto-fit whenever the active layout's size changes (design: "auto-fits on
  // open/layout-change"). happy-dom/jsdom report 0 client size in tests, so a
  // zero-size guard keeps this a no-op there (zoom stays at its default 1,
  // preserving GraphScreen's existing position assertions).
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el || layout.width <= 0 || layout.height <= 0) return;
    const availW = el.clientWidth - CANVAS_PAD * 2;
    const availH = el.clientHeight - CANVAS_PAD * 2;
    if (availW <= 0 || availH <= 0) return;
    const z = Math.max(ZOOM_MIN, Math.min(availW / layout.width, availH / layout.height, 1.4));
    setZoom(z);
    // Re-run whenever the canvas content size changes (new layout / mode).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [layout.width, layout.height, layoutMode]);

  function fitView(): void {
    const el = scrollRef.current;
    if (!el || layout.width <= 0 || layout.height <= 0) return;
    const availW = el.clientWidth - CANVAS_PAD * 2;
    const availH = el.clientHeight - CANVAS_PAD * 2;
    if (availW <= 0 || availH <= 0) {
      setZoom(1);
      return;
    }
    setZoom(Math.max(ZOOM_MIN, Math.min(availW / layout.width, availH / layout.height, 1.4)));
  }

  function setClampedZoom(z: number): void {
    setZoom(Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, z)));
  }

  // ---- Drag-to-connect (design mode) --------------------------------------
  // Measured against the SCALED canvas rect, so it stays accurate at any
  // zoom level (design: "measured against the scaled canvas rect").
  function toCanvasPoint(clientX: number, clientY: number): Point {
    const el = canvasRef.current;
    if (!el) return { x: 0, y: 0 };
    const r = el.getBoundingClientRect();
    const z = zoom || 1;
    return { x: (clientX - r.left) / z, y: (clientY - r.top) / z };
  }

  function onHandleDown(nodeId: string, e: ReactMouseEvent): void {
    e.preventDefault();
    e.stopPropagation();
    const p = toCanvasPoint(e.clientX, e.clientY);
    setDrag({ from: nodeId, x: p.x, y: p.y });
  }

  useEffect(() => {
    if (!drag) return;
    function onMove(e: MouseEvent): void {
      const p = toCanvasPoint(e.clientX, e.clientY);
      setDrag((d) => (d ? { ...d, x: p.x, y: p.y } : d));
    }
    function onUp(e: MouseEvent): void {
      const el = document.elementFromPoint(e.clientX, e.clientY);
      const hostEl = el && (el as HTMLElement).closest ? (el as HTMLElement).closest("[data-node-id]") : null;
      const toId = hostEl ? hostEl.getAttribute("data-node-id") : null;
      setDrag((cur) => {
        if (cur && toId && toId !== cur.from) {
          setEdgePick({ from: cur.from, to: toId });
        }
        return null;
      });
    }
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [drag, zoom]);

  function pickEdgeKind(kind: ConnKind): void {
    if (edgePick) {
      onConnect?.(edgePick.from, edgePick.to, kind, false);
    }
    setEdgePick(null);
  }

  // ---- Neighbourhood highlight (selection dims everything else) ----------
  const neighbourhood = new Set<string>();
  if (selectedNode) {
    neighbourhood.add(selectedNode);
    for (const e of graph.edges) {
      if (e.from === selectedNode) neighbourhood.add(e.to);
      if (e.to === selectedNode) neighbourhood.add(e.from);
    }
  }

  const canvasEmpty = graph.nodes.length === 0;

  return (
    <div
      style={{
        display: "flex",
        height: "100%",
        minHeight: 520,
        color: "var(--txt)",
      }}
    >
      {/* ================= Canvas column ================= */}
      <div style={{ position: "relative", flex: 1, minWidth: 0, overflow: "hidden" }}>
        {/* Scroll/pan container */}
        <div
          ref={scrollRef}
          data-testid="graph-scroll"
          style={{
            position: "absolute",
            inset: 0,
            overflow: "auto",
          }}
        >
          <div
            style={{
              minWidth: "100%",
              minHeight: "100%",
              display: "flex",
              alignItems: "flex-start",
              justifyContent: "center",
              padding: CANVAS_PAD,
              boxSizing: "border-box",
            }}
          >
            {/* Zoom wrapper: transform:scale(z), origin top-left */}
            <div
              style={{
                width: layout.width * zoom,
                height: layout.height * zoom,
                flexShrink: 0,
              }}
            >
              <div
                ref={canvasRef}
                data-testid="graph-canvas"
                data-layout-mode={layoutMode}
                style={{
                  position: "relative",
                  width: layout.width,
                  height: layout.height,
                  transform: `scale(${zoom})`,
                  transformOrigin: "top left",
                  backgroundImage:
                    "radial-gradient(circle, rgba(255,255,255,0.04) 1px, transparent 1px)",
                  backgroundSize: "26px 26px",
                }}
              >
                {/* Host swimlane zone boxes (By-host mode only) */}
                {layoutMode === "host" &&
                  layout.hosts.map((host, col) => (
                    <div
                      key={host}
                      data-testid={`host-lane-${host}`}
                      style={{
                        position: "absolute",
                        left: col * (LANE_W + LANE_GAP),
                        top: 0,
                        width: LANE_W,
                        height: layout.height,
                        borderRadius: 16,
                        border: "1px solid var(--border)",
                        background: "rgba(21,25,33,0.5)",
                        backdropFilter: "blur(10px)",
                        WebkitBackdropFilter: "blur(10px)",
                        overflow: "hidden",
                        pointerEvents: "none",
                      }}
                    >
                      <div
                        className="mono"
                        style={{
                          padding: "8px 10px",
                          fontSize: "calc(10.5px * var(--fs,1))",
                          fontWeight: 700,
                          letterSpacing: 0.4,
                          color: "var(--txt3)",
                          textTransform: "uppercase",
                        }}
                      >
                        ▪ {host}
                      </div>
                    </div>
                  ))}

                {/* Edge layer */}
                <svg
                  width={layout.width}
                  height={layout.height}
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
                    const bi = graphEdge?.bi ?? false;
                    const color = edgeColor(edge.status);
                    const dash = edgeDashStyle(kind);
                    const width = edgeStrokeWidth(kind, edge.status);
                    const isDimmed = selectedNode != null && !(neighbourhood.has(edge.from) && neighbourhood.has(edge.to));
                    const target = edge.points[edge.points.length - 1];
                    const source = edge.points[0];
                    return (
                      <g key={edge.id} opacity={isDimmed ? 0.16 : 1}>
                        <path
                          data-testid={`graph-edge-${edge.id}`}
                          data-status={edge.status}
                          data-kind={kind}
                          d={smoothEdgePath(edge.points)}
                          fill="none"
                          stroke={color}
                          strokeWidth={width}
                          strokeDasharray={dash.dasharray}
                          strokeLinecap={dash.linecap}
                          style={dash.animated ? { animation: "flow 1.1s linear infinite" } : undefined}
                        />
                        {/* Arrowhead into the target (always) */}
                        <polygon
                          points={`${target.x - 5},${target.y - 8} ${target.x + 5},${target.y - 8} ${target.x},${target.y}`}
                          fill={color}
                        />
                        {/* Arrowhead into the source too, for consulting/bi edges */}
                        {bi && (
                          <polygon
                            points={`${source.x - 5},${source.y + 8} ${source.x + 5},${source.y + 8} ${source.x},${source.y}`}
                            fill={color}
                          />
                        )}
                      </g>
                    );
                  })}
                </svg>

                {/* Gate diamonds (overlay divs, above the edge svg) */}
                {layout.edges.map((edge) => {
                  if (!edge.gate) return null;
                  const isDimmed =
                    selectedNode != null && !(neighbourhood.has(edge.from) && neighbourhood.has(edge.to));
                  return (
                    <div
                      key={`gate-${edge.id}`}
                      data-testid={`gate-diamond-${edge.id}`}
                      role="button"
                      title="Governed handoff — click to review"
                      onClick={(ev) => {
                        ev.stopPropagation();
                        onOpenGate(edge.id);
                      }}
                      style={{
                        position: "absolute",
                        left: edge.gate.x - 15,
                        top: edge.gate.y - 15,
                        width: 30,
                        height: 30,
                        zIndex: 4,
                        cursor: "pointer",
                        opacity: isDimmed ? 0.3 : 1,
                        display: "flex",
                        alignItems: "center",
                        justifyContent: "center",
                      }}
                    >
                      <div
                        style={{
                          width: 30,
                          height: 30,
                          borderRadius: 9,
                          transform: "rotate(45deg)",
                          background: "var(--needs-bg)",
                          border: "1px solid var(--needs)",
                          display: "flex",
                          alignItems: "center",
                          justifyContent: "center",
                          animation: "glow 2.6s ease-in-out infinite",
                        }}
                      >
                        <span
                          style={{
                            transform: "rotate(-45deg)",
                            color: "var(--needs)",
                            fontSize: 13,
                            fontWeight: 800,
                          }}
                        >
                          ▲
                        </span>
                      </div>
                    </div>
                  );
                })}

                {/* Rubber-band drag-connect line (design mode) */}
                {drag && (
                  <svg
                    style={{ position: "absolute", left: 0, top: 0, pointerEvents: "none", overflow: "visible" }}
                    width={layout.width}
                    height={layout.height}
                  >
                    <line
                      data-testid="drag-connect-line"
                      x1={drag.x}
                      y1={drag.y}
                      x2={drag.x}
                      y2={drag.y}
                      stroke="var(--accent)"
                      strokeWidth={2.5}
                      strokeDasharray="6 5"
                    />
                  </svg>
                )}

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
                      data-node-id={n.id}
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
                        background: "rgba(26,31,41,0.94)",
                        borderTop: `1px solid ${isSelected ? color : "var(--border2)"}`,
                        borderRight: `1px solid ${isSelected ? color : "var(--border2)"}`,
                        borderBottom: `1px solid ${isSelected ? color : "var(--border2)"}`,
                        borderLeft: `3px solid ${color}`,
                        borderRadius: 13,
                        padding: "9px 12px",
                        cursor: "pointer",
                        opacity: isDimmed ? 0.34 : 1,
                        boxShadow: isSelected ? "0 0 18px var(--accent-bg)" : "var(--shadow-sm)",
                        overflow: "hidden",
                        display: "flex",
                        flexDirection: "column",
                        gap: 4,
                        zIndex: 2,
                      }}
                    >
                      {/* Header row: state dot + name + open affordance */}
                      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                        <span
                          data-pulse={pulse ? "true" : "false"}
                          style={{
                            width: 8,
                            height: 8,
                            borderRadius: "50%",
                            background: color,
                            display: "inline-block",
                            flexShrink: 0,
                            animation: pulse ? "dotpulse 1.3s infinite" : undefined,
                          }}
                        />
                        <span
                          style={{
                            fontWeight: 600,
                            fontSize: "calc(12.5px * var(--fs,1))",
                            whiteSpace: "nowrap",
                            overflow: "hidden",
                            textOverflow: "ellipsis",
                            flex: 1,
                          }}
                        >
                          {n.name}
                        </span>
                      </div>

                      {/* Meta row: driver pill + (Flow) host chip + status + chevron */}
                      <div style={{ display: "flex", alignItems: "center", gap: 7 }}>
                        <span
                          data-testid="driver-pill"
                          data-driver={n.driver}
                          style={{
                            fontSize: "calc(9.5px * var(--fs,1))",
                            fontWeight: 700,
                            padding: "2px 7px",
                            borderRadius: 5,
                            color: driverColor[n.driver],
                            background: `${driverColor[n.driver]}22`,
                            border: `1px solid ${driverColor[n.driver]}`,
                            flexShrink: 0,
                          }}
                        >
                          {n.driver}
                        </span>
                        {layoutMode === "flow" && (
                          <span
                            className="mono"
                            data-testid={`node-host-chip-${n.id}`}
                            style={{
                              fontSize: "calc(9px * var(--fs,1))",
                              color: "var(--txt3)",
                              whiteSpace: "nowrap",
                              flexShrink: 0,
                            }}
                          >
                            ▪ {n.host}
                          </span>
                        )}
                        <span
                          style={{
                            fontSize: "calc(9.5px * var(--fs,1))",
                            color: "var(--txt3)",
                            whiteSpace: "nowrap",
                            overflow: "hidden",
                            textOverflow: "ellipsis",
                            flex: 1,
                          }}
                        >
                          {n.status}
                        </span>
                        {!designMode && (
                          <span
                            data-testid={`node-open-${n.id}`}
                            role="button"
                            title="Open session"
                            onClick={(ev) => {
                              ev.stopPropagation();
                              onOpenNode(n.id);
                            }}
                            style={{ color: "var(--txt3)", cursor: "pointer", flexShrink: 0, fontSize: 13 }}
                          >
                            ›
                          </span>
                        )}
                      </div>

                      {/* Connect handle (design mode only) */}
                      {designMode && (
                        <div
                          data-testid={`connect-handle-${n.id}`}
                          title="Drag to connect"
                          onMouseDown={(e) => onHandleDown(n.id, e)}
                          style={{
                            position: "absolute",
                            left: "50%",
                            bottom: -8,
                            transform: "translateX(-50%)",
                            width: 16,
                            height: 16,
                            borderRadius: "50%",
                            background: "var(--accent)",
                            border: "2px solid var(--panel)",
                            cursor: "crosshair",
                            zIndex: 3,
                          }}
                        />
                      )}
                    </div>
                  );
                })}
              </div>
            </div>
          </div>
        </div>

        {/* Empty state */}
        {canvasEmpty && (
          <div
            data-testid="graph-empty"
            style={{
              position: "absolute",
              inset: 0,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              pointerEvents: "none",
            }}
          >
            <div style={{ textAlign: "center", color: "var(--txt3)" }}>
              <div style={{ fontSize: 32, marginBottom: 10, opacity: 0.5 }}>⛓</div>
              <div style={{ fontSize: "calc(13px * var(--fs,1))", fontWeight: 600, color: "var(--txt2)" }}>
                No agents yet
              </div>
              {designMode && (
                <div style={{ fontSize: "calc(11.5px * var(--fs,1))", marginTop: 3 }}>
                  Click ＋ Add agent to place your first one
                </div>
              )}
            </div>
          </div>
        )}

        {/* Edge legend (top-left) */}
        <div
          data-testid="edge-legend"
          style={{
            position: "absolute",
            left: 14,
            top: 14,
            zIndex: 6,
            display: "flex",
            flexDirection: "column",
            gap: 5,
            background: "var(--elev)",
            border: "1px solid var(--border2)",
            borderRadius: 9,
            padding: "9px 11px",
            boxShadow: "var(--shadow-sm)",
            fontSize: "calc(10.5px * var(--fs,1))",
            color: "var(--txt2)",
          }}
        >
          <LegendRow color="var(--accent)" dash="8 5" label="Data / message" />
          <LegendRow color="var(--accent)" dash="2 4" label="Command / prompt" />
          <LegendRow color="var(--accent)" dash={undefined} label="Both" />
          <LegendRow color="var(--txt2)" dash={undefined} label="Consulting (two-way)" biArrows />
          <LegendRow color="var(--needs)" dash={undefined} label="Needs attention" />
        </div>

        {/* ＋ Add agent / ✎ Update workflow (above canvas) */}
        {designMode ? (
          <button
            data-testid="add-agent-btn"
            onClick={() => onAddAgent?.()}
            style={{
              position: "absolute",
              left: "50%",
              top: 14,
              transform: "translateX(-50%)",
              zIndex: 6,
              fontFamily: "inherit",
              fontWeight: 700,
              fontSize: "calc(12px * var(--fs,1))",
              padding: "8px 15px",
              borderRadius: 9,
              border: "none",
              background: "var(--accent)",
              color: "#fff",
              cursor: "pointer",
              boxShadow: "var(--shadow-sm)",
            }}
          >
            ＋ Add agent
          </button>
        ) : (
          <button
            data-testid="enter-design-btn"
            onClick={() => onEnterDesign?.()}
            style={{
              position: "absolute",
              left: "50%",
              top: 14,
              transform: "translateX(-50%)",
              zIndex: 6,
              fontFamily: "inherit",
              fontWeight: 600,
              fontSize: "calc(12px * var(--fs,1))",
              padding: "8px 15px",
              borderRadius: 9,
              border: "1px solid var(--border2)",
              background: "var(--elev)",
              color: "var(--txt2)",
              cursor: "pointer",
              boxShadow: "var(--shadow-sm)",
            }}
          >
            ✎ Update workflow
          </button>
        )}

        {/* Layout toggle (bottom-left) */}
        <div
          style={{
            position: "absolute",
            left: 14,
            bottom: 14,
            zIndex: 6,
            display: "flex",
            background: "var(--elev)",
            border: "1px solid var(--border2)",
            borderRadius: 9,
            padding: 3,
            boxShadow: "var(--shadow-sm)",
          }}
        >
          <button
            data-testid="layout-toggle-host"
            onClick={() => setLayoutMode("host")}
            style={{
              fontFamily: "inherit",
              fontSize: "calc(11.5px * var(--fs,1))",
              fontWeight: 600,
              padding: "5px 11px",
              borderRadius: 7,
              cursor: "pointer",
              border: "none",
              background: layoutMode === "host" ? "var(--accent)" : "transparent",
              color: layoutMode === "host" ? "#fff" : "var(--txt2)",
            }}
          >
            By host
          </button>
          <button
            data-testid="layout-toggle-flow"
            onClick={() => setLayoutMode("flow")}
            style={{
              fontFamily: "inherit",
              fontSize: "calc(11.5px * var(--fs,1))",
              fontWeight: 600,
              padding: "5px 11px",
              borderRadius: 7,
              cursor: "pointer",
              border: "none",
              background: layoutMode === "flow" ? "var(--accent)" : "transparent",
              color: layoutMode === "flow" ? "#fff" : "var(--txt2)",
            }}
          >
            Flow
          </button>
        </div>

        {/* Zoom controls (bottom-right) */}
        <div
          style={{
            position: "absolute",
            right: 14,
            bottom: 14,
            zIndex: 6,
            display: "flex",
            alignItems: "center",
            gap: 4,
            background: "var(--elev)",
            border: "1px solid var(--border2)",
            borderRadius: 9,
            padding: 4,
            boxShadow: "var(--shadow-sm)",
          }}
        >
          <button
            data-testid="zoom-out"
            onClick={() => setClampedZoom(zoom / 1.2)}
            style={{
              fontFamily: "inherit",
              width: 28,
              height: 28,
              borderRadius: 7,
              border: "none",
              background: "var(--panel2)",
              color: "var(--txt2)",
              cursor: "pointer",
              fontSize: 16,
              lineHeight: 1,
            }}
          >
            −
          </button>
          <span
            data-testid="zoom-pct"
            className="mono"
            onClick={() => setClampedZoom(1)}
            style={{ fontSize: "calc(11px * var(--fs,1))", color: "var(--txt2)", minWidth: 42, textAlign: "center", cursor: "pointer" }}
          >
            {Math.round(zoom * 100)}%
          </span>
          <button
            data-testid="zoom-in"
            onClick={() => setClampedZoom(zoom * 1.2)}
            style={{
              fontFamily: "inherit",
              width: 28,
              height: 28,
              borderRadius: 7,
              border: "none",
              background: "var(--panel2)",
              color: "var(--txt2)",
              cursor: "pointer",
              fontSize: 15,
              lineHeight: 1,
            }}
          >
            ＋
          </button>
          <button
            data-testid="zoom-fit"
            onClick={fitView}
            style={{
              fontFamily: "inherit",
              fontSize: "calc(11px * var(--fs,1))",
              fontWeight: 600,
              padding: "0 10px",
              height: 28,
              borderRadius: 7,
              border: "none",
              background: "var(--panel2)",
              color: "var(--txt2)",
              cursor: "pointer",
            }}
          >
            Fit
          </button>
        </div>

        {/* Edge-kind picker (drag-connect drop) */}
        {edgePick && (
          <div
            onClick={() => setEdgePick(null)}
            style={{
              position: "fixed",
              inset: 0,
              zIndex: 62,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              background: "rgba(5,7,10,0.4)",
            }}
          >
            <div
              onClick={(e) => e.stopPropagation()}
              data-testid="edge-kind-picker"
              style={{
                background: "var(--elev)",
                border: "1px solid var(--border2)",
                borderRadius: 14,
                boxShadow: "var(--shadow)",
                padding: "16px 18px",
                animation: "slideup .15s ease",
              }}
            >
              <div style={{ fontSize: "calc(12.5px * var(--fs,1))", fontWeight: 700, marginBottom: 3 }}>
                Connect {edgePick.from} → {edgePick.to}
              </div>
              <div style={{ fontSize: "calc(11px * var(--fs,1))", color: "var(--txt3)", marginBottom: 13 }}>
                What kind of connection?
              </div>
              <div style={{ display: "flex", gap: 8 }}>
                <button
                  data-testid="edge-kind-data"
                  onClick={() => pickEdgeKind("data")}
                  style={pickerBtnStyle("var(--accent)", "var(--accent-bg)", "var(--accent2)")}
                >
                  Data / message
                </button>
                <button
                  data-testid="edge-kind-command"
                  onClick={() => pickEdgeKind("command")}
                  style={pickerBtnStyle("var(--txt2)", "transparent", "var(--txt2)")}
                >
                  Command
                </button>
                <button
                  data-testid="edge-kind-both"
                  onClick={() => pickEdgeKind("both")}
                  style={pickerBtnStyle("var(--border2)", "var(--panel2)", "var(--txt2)")}
                >
                  Both
                </button>
              </div>
            </div>
          </div>
        )}
      </div>

      {/* ================= Inspector ================= */}
      <aside
        data-testid="graph-inspector"
        style={{
          width: 320,
          flexShrink: 0,
          borderLeft: "1px solid var(--border)",
          background: "rgba(18,22,29,0.74)",
          backdropFilter: "blur(22px)",
          WebkitBackdropFilter: "blur(22px)",
          overflowY: "auto",
          padding: 18,
        }}
      >
        <div
          style={{
            fontSize: "calc(10px * var(--fs,1))",
            fontWeight: 700,
            letterSpacing: 0.6,
            textTransform: "uppercase",
            color: "var(--txt3)",
          }}
        >
          Workstream
        </div>
        <div style={{ fontSize: "calc(17px * var(--fs,1))", fontWeight: 700, letterSpacing: -0.2, marginTop: 4 }}>
          {graph.name}
        </div>
        <div style={{ fontSize: "calc(11.5px * var(--fs,1))", color: "var(--txt3)", marginTop: 3 }}>{graph.goal}</div>

        {designMode ? (
          <>
            <div
              style={{
                margin: "15px 0",
                padding: 13,
                borderRadius: 12,
                border: "1px dashed var(--accent)",
                background: "var(--accent-bg)",
              }}
            >
              <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 5 }}>
                <span style={{ color: "var(--accent2)" }}>✎</span>
                <span style={{ fontWeight: 700, fontSize: "calc(12.5px * var(--fs,1))", color: "var(--accent2)" }}>
                  Design mode
                </span>
              </div>
              <div style={{ fontSize: "calc(11.5px * var(--fs,1))", color: "var(--txt)", lineHeight: 1.5 }}>
                Place agents and wire them together. Nothing runs until you confirm.
              </div>
              <div style={{ display: "flex", gap: 8, marginTop: 12 }}>
                <button
                  data-testid="confirm-draft-btn"
                  onClick={() => onConfirmDraft?.()}
                  style={{
                    flex: 1,
                    fontFamily: "inherit",
                    fontWeight: 700,
                    fontSize: "calc(12px * var(--fs,1))",
                    padding: 9,
                    borderRadius: 9,
                    border: "none",
                    background: "var(--done)",
                    color: "#0e1116",
                    cursor: "pointer",
                  }}
                >
                  Confirm &amp; launch
                </button>
                <button
                  data-testid="discard-draft-btn"
                  onClick={() => onDiscardDraft?.()}
                  style={{
                    fontFamily: "inherit",
                    fontWeight: 600,
                    fontSize: "calc(12px * var(--fs,1))",
                    padding: "9px 13px",
                    borderRadius: 9,
                    border: "1px solid var(--border2)",
                    background: "var(--panel2)",
                    color: "var(--txt2)",
                    cursor: "pointer",
                  }}
                >
                  Discard
                </button>
              </div>
            </div>
            <div
              style={{
                fontSize: "calc(10px * var(--fs,1))",
                fontWeight: 700,
                letterSpacing: 0.6,
                textTransform: "uppercase",
                color: "var(--txt3)",
                marginBottom: 9,
              }}
            >
              Agents ({graph.nodes.length})
            </div>
            <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
              {graph.nodes.map((n) => (
                <div
                  key={n.id}
                  data-testid={`design-agent-${n.id}`}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 8,
                    padding: "6px 8px",
                    borderRadius: 7,
                    background: "var(--panel2)",
                    border: "1px solid var(--border)",
                    fontSize: "calc(12px * var(--fs,1))",
                  }}
                >
                  <span
                    style={{
                      width: 7,
                      height: 7,
                      borderRadius: "50%",
                      background: driverColor[n.driver],
                      flexShrink: 0,
                    }}
                  />
                  <span style={{ flex: 1, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
                    {n.name}
                  </span>
                </div>
              ))}
            </div>
          </>
        ) : (
          <>
            <div style={{ display: "flex", gap: 8, margin: "15px 0" }}>
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

            {/* Workstream lifecycle (#5) — Pause all / Resume all / Delete. */}
            {(onPauseWorkstream || onResumeWorkstream || onDeleteWorkstream) && (
              <div style={{ display: "flex", gap: 7, marginBottom: 14, flexWrap: "wrap" }}>
                {onPauseWorkstream && (
                  <button
                    data-testid="inspector-pause-ws"
                    onClick={() => onPauseWorkstream()}
                    style={inspectorActionStyle(false)}
                  >
                    ⏸ Pause all
                  </button>
                )}
                {onResumeWorkstream && (
                  <button
                    data-testid="inspector-resume-ws"
                    onClick={() => onResumeWorkstream()}
                    style={inspectorActionStyle(false)}
                  >
                    ▶ Resume all
                  </button>
                )}
                {onDeleteWorkstream && (
                  <button
                    data-testid="inspector-delete-ws"
                    onClick={() => onDeleteWorkstream()}
                    style={inspectorActionStyle(true)}
                  >
                    ✕ Delete
                  </button>
                )}
              </div>
            )}

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
                  marginBottom: 14,
                }}
              >
                <div style={{ fontWeight: 700 }}>Governed handoff</div>
                <div style={{ color: "var(--txt2)", marginTop: 2 }}>
                  {graph.inspector.pending_handoff.from} → {graph.inspector.pending_handoff.to}
                </div>
                <div style={{ color: "var(--txt3)", marginTop: 2 }}>{graph.inspector.pending_handoff.payload}</div>
              </div>
            )}

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
                      padding: "9px 11px",
                      borderRadius: 9,
                      background: "var(--panel2)",
                      border: "1px solid var(--border)",
                      cursor: "pointer",
                      fontSize: "calc(12px * var(--fs,1))",
                    }}
                  >
                    <span
                      data-pulse={pulsesFor(n.state) ? "true" : "false"}
                      style={{
                        width: 7,
                        height: 7,
                        borderRadius: "50%",
                        background: stateColor(n.state),
                        display: "inline-block",
                        flexShrink: 0,
                        animation: pulsesFor(n.state) ? "dotpulse 1.3s infinite" : undefined,
                      }}
                    />
                    <span style={{ flex: 1, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
                      {n.name}
                    </span>
                    <span style={{ color: "var(--txt3)" }}>›</span>
                  </div>
                ))}
              </div>
            </div>
          </>
        )}
      </aside>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Small local helpers
// ---------------------------------------------------------------------------

function inspectorActionStyle(danger: boolean): CSSProperties {
  return {
    fontFamily: "inherit",
    fontSize: "calc(11.5px * var(--fs,1))",
    fontWeight: 600,
    padding: "6px 11px",
    borderRadius: 8,
    cursor: "pointer",
    border: danger ? "1px solid var(--error)" : "1px solid var(--border2)",
    background: danger ? "var(--error-bg)" : "var(--panel2)",
    color: danger ? "var(--error)" : "var(--txt2)",
    whiteSpace: "nowrap",
  };
}

function pickerBtnStyle(border: string, bg: string, fg: string): CSSProperties {
  return {
    fontFamily: "inherit",
    fontWeight: 700,
    fontSize: "calc(12px * var(--fs,1))",
    padding: "9px 14px",
    borderRadius: 9,
    border: `1px solid ${border}`,
    background: bg,
    color: fg,
    cursor: "pointer",
  };
}

function LegendRow(props: { color: string; dash: string | undefined; label: string; biArrows?: boolean }): JSX.Element {
  const { color, dash, label, biArrows } = props;
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
      <svg width="28" height="9">
        <line
          x1={biArrows ? 5 : 1}
          y1="4.5"
          x2={biArrows ? 23 : 22}
          y2="4.5"
          style={{ stroke: color, strokeWidth: 2, strokeDasharray: dash }}
        />
        <path d="M22 1 L27 4.5 L22 8 z" fill={biArrows ? "none" : color} />
        {biArrows && (
          <>
            <path d="M5 1.5 L1 4.5 L5 7.5" fill="none" stroke={color} strokeWidth={1.4} />
            <path d="M23 1.5 L27 4.5 L23 7.5" fill="none" stroke={color} strokeWidth={1.4} />
          </>
        )}
      </svg>
      {label}
    </div>
  );
}
