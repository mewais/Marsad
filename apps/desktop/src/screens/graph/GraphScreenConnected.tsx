/**
 * GraphScreenConnected — connected wrapper for the Level-2 workstream graph.
 *
 * Consumes `useVm()` and passes props down to the presentational `GraphScreen`.
 * No prop-drilling from App; the wrapper owns the state binding.
 *
 * Design-mode authoring ("Build it out") is a PURE CLIENT-SIDE DRAFT: authored
 * nodes/edges live in the store (`state.draftNodes` / `state.draftEdges`, NOT
 * component-local state — the authoring flow navigates graph → node-modal →
 * graph, which unmounts this wrapper on every add). Nothing hits the real
 * `marsad-core` until "Confirm & launch" (DESIGN_HANDOFF.md:107,160,183):
 *   - "＋ Add agent" opens the node modal in design mode; the modal appends a
 *     DraftNode (no backend call). A real session id only exists post-launch
 *     (core.md:29), so a draft node has none.
 *   - `onConnect` between two DRAFT (temp) nodes records a DraftEdge — it must
 *     NOT call the real `addConnection`, whose endpoints must be real placed
 *     nodes (core-ext.md:69-72). A wire touching a real node still goes straight
 *     to the backend.
 *   - `onConfirmDraft` is the ONE materialization point: launch each draft node
 *     (lead first) → assign with the returned REAL id → then submit each draft
 *     edge with the temp→real-resolved endpoint ids.
 *   - `onDiscardDraft` clears the draft with no backend calls (non-empty draft);
 *     the pre-existing empty-workstream cleanup (openBoard) is preserved.
 *
 * See docs/specs/ui-app-wiring.md §Connected wrappers.
 */
import { useEffect, useRef, useState } from "react";
import { useVm } from "../../bridge/useVm";
import type { DraftNode } from "../../bridge/useVm";
import { launchSession } from "../../bridge/api";
import type { GraphNode, GraphView } from "../../bridge/workstreamTypes";
import { GraphScreen } from "./GraphScreen";

/** Local id generator for a newly drag-connected edge — mirrors the design
 *  reference's `'e'+Date.now()` (Marsad Workstreams.dc.html ~line 897).
 *  `crypto.randomUUID` is preferred when available (Tauri's webview has it);
 *  falls back to a timestamp so this never throws in an older environment. */
function genConnId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return `conn-${crypto.randomUUID()}`;
  }
  return `conn-${Date.now()}`;
}

/** Render a draft (unlaunched) node as a GraphNode so it shows on the design
 *  canvas with a connect handle + a `design-agent-<tempId>` inspector row. It
 *  carries no live session state yet — it's a placeholder pending launch. */
function draftToGraphNode(d: DraftNode): GraphNode {
  return {
    id: d.tempId,
    name: d.name,
    driver: d.driver,
    host: d.hostId,
    state: "Starting",
    status: "Draft — not launched",
    context_pct: 0,
    is_lead: d.isLead,
    depth: 0,
  };
}

export function GraphScreenConnected(): JSX.Element {
  const {
    state,
    selectNode,
    openNode,
    addConnection,
    addDraftEdge,
    clearDraft,
    assignSession,
    goToGraph,
    openNodeModal,
    pauseWorkstream,
    resumeWorkstream,
    deleteWorkstream,
    openBoard,
  } = useVm();
  // #3/#4 — design-mode entry (THE launch unblocker). A workstream that is
  // NEW or has ZERO sessions must open in DESIGN mode so "＋ Add agent" is
  // shown. Once a workstream has sessions it opens in operate mode; the user
  // can still flip back to design via "✎ Update workflow".
  const [designMode, setDesignMode] = useState<boolean | null>(null);
  const seededWsRef = useRef<string | null>(null);

  const graph = state.graph;
  const wsId = (state.wsId ?? graph?.ws_id) ?? null;

  useEffect(() => {
    if (!graph || wsId == null) return;
    if (seededWsRef.current === wsId) return;
    seededWsRef.current = wsId;
    // Empty / new workstream ⇒ start in design mode.
    setDesignMode(graph.nodes.length === 0);
  }, [wsId, graph]);

  if (graph == null) {
    return <div data-testid="graph-loading" />;
  }
  const realGraph = graph; // non-null binding for the closures below

  const effectiveWsId = wsId ?? realGraph.ws_id;
  const draftNodes = state.draftNodes;
  const draftEdges = state.draftEdges;

  // Merge the client-side draft nodes/edges with the real backend graph for
  // DISPLAY, so authored-but-unlaunched nodes render (connect handles +
  // design-agent rows) alongside any already-real ones. This synthetic view is
  // never sent to the backend — it only drives the canvas.
  const g: GraphView = {
    ...realGraph,
    nodes: [...realGraph.nodes, ...draftNodes.map(draftToGraphNode)],
    edges: [
      ...realGraph.edges,
      ...draftEdges.map((e) => ({
        id: e.tempId,
        from: e.from,
        to: e.to,
        kind: e.kind,
        bi: e.bi,
        status: "normal" as const,
        has_gate: false,
      })),
    ],
  };

  // Design mode when the store still has a draft in flight, when the backend
  // graph is empty, or when the user explicitly toggled in.
  const effectiveDesignMode =
    designMode ?? (realGraph.nodes.length === 0 || draftNodes.length > 0);

  const draftTempIds = new Set(draftNodes.map((d) => d.tempId));

  // Interim gate wiring: route the gate affordance to the RECEIVING node's
  // session (the dedicated handoff-approval modal is a later block).
  function onOpenGate(connId: string): void {
    const edge = g.edges.find((x) => x.id === connId);
    if (edge) {
      openNode(edge.to);
    }
  }

  // "＋ Add agent" — opens the node/add-agent modal in DESIGN mode to author a
  // node into the draft workstream graph. Always a design-mode add (first node
  // and subsequent ones), so the modal records a DraftNode and NEVER launches a
  // standalone single session. `isFirstNode` tracks whether the draft is still
  // empty (drives lead-assignment + wiring-picker visibility).
  function onAddAgent(): void {
    openNodeModal({
      wsId: effectiveWsId,
      wsName: realGraph.name,
      isFirstNode: realGraph.nodes.length === 0 && draftNodes.length === 0,
      designMode: true,
    });
  }

  // Drag-to-connect result. In design mode, a wire whose endpoint is a DRAFT
  // (temp) node cannot be a real connection (its endpoints aren't launched
  // sessions yet — core-ext.md:69-72); record a draft edge instead. A wire
  // between two REAL nodes goes straight to the backend as before.
  function onConnect(
    from: string,
    to: string,
    kind: Parameters<typeof addConnection>[4],
    bi: boolean
  ): void {
    if (effectiveDesignMode && (draftTempIds.has(from) || draftTempIds.has(to))) {
      addDraftEdge({ from, to, kind, bi });
      return;
    }
    void addConnection(genConnId(), effectiveWsId, from, to, kind, bi);
  }

  // "Confirm & launch" — the ONE backend materialization point. Launch every
  // draft node (LEAD FIRST), assign each with the REAL id launchSession returns,
  // building a temp→real map; then submit each draft edge with the resolved
  // real endpoint ids. Finally clear the draft + leave design mode and refresh
  // the graph so the now-real nodes render.
  async function onConfirmDraft(): Promise<void> {
    if (draftNodes.length === 0) {
      // No draft authored — just leave design mode (operate-mode "done").
      setDesignMode(false);
      return;
    }
    // Lead first, then the rest in authored order (draft node #0 is the lead).
    const ordered = [...draftNodes].sort(
      (a, b) => Number(b.isLead) - Number(a.isLead)
    );
    const tempToReal = new Map<string, string>();
    try {
      for (const node of ordered) {
        const realId = await launchSession({
          project: node.project,
          host_id: node.hostId,
          model: node.model,
          posture: node.posture,
          first_prompt: node.firstPrompt,
          name: node.name,
          driver: node.driver,
        });
        await assignSession(realId, effectiveWsId, node.isLead);
        tempToReal.set(node.tempId, realId);
      }
      for (const edge of draftEdges) {
        const from = tempToReal.get(edge.from) ?? edge.from;
        const to = tempToReal.get(edge.to) ?? edge.to;
        await addConnection(genConnId(), effectiveWsId, from, to, edge.kind, edge.bi);
      }
      clearDraft();
      setDesignMode(false);
      // Refresh the graph so the freshly-launched real nodes replace the drafts.
      await goToGraph(effectiveWsId);
    } catch (err) {
      console.error("[GraphScreenConnected] confirm & launch failed:", err);
    }
  }

  // "Discard" in the design panel.
  // - A non-empty draft (nodes authored, not confirmed): clear the draft with
  //   NO backend calls — the draft nodes never became real sessions.
  // - A fresh, empty "Build it out" draft (0 authored nodes / no session yet):
  //   abandon the whole workstream — route back to the Board, where `openBoard`
  //   discards the pending draft so no orphan empty workstream lingers.
  // - Otherwise (a real workstream with nodes): just leave design mode.
  function onDiscardDraft(): void {
    if (draftNodes.length > 0) {
      clearDraft();
      setDesignMode(false);
      return;
    }
    if (realGraph.nodes.length === 0 && state.draftWsId === effectiveWsId) {
      openBoard();
    } else {
      setDesignMode(false);
    }
  }

  function onDeleteWorkstream(): void {
    const label = realGraph.name || effectiveWsId;
    const ok =
      typeof window === "undefined" || typeof window.confirm !== "function"
        ? true
        : window.confirm(
            `Delete workstream "${label}"? This kills all its sessions and cannot be undone.`
          );
    if (!ok) return;
    // Deleting the open workstream — return to the board afterwards.
    void deleteWorkstream(effectiveWsId).then(() => openBoard());
  }

  return (
    <GraphScreen
      graph={g}
      hosts={state.snapshot?.hosts ?? []}
      selectedNode={state.nodeSel}
      onSelectNode={selectNode}
      onOpenNode={openNode}
      onOpenGate={onOpenGate}
      designMode={effectiveDesignMode}
      draftCount={draftNodes.length}
      onEnterDesign={() => setDesignMode(true)}
      onConfirmDraft={() => void onConfirmDraft()}
      onDiscardDraft={onDiscardDraft}
      onAddAgent={onAddAgent}
      onConnect={onConnect}
      onPauseWorkstream={() => void pauseWorkstream(effectiveWsId)}
      onResumeWorkstream={() => void resumeWorkstream(effectiveWsId)}
      onDeleteWorkstream={onDeleteWorkstream}
    />
  );
}
