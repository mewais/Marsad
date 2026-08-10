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
import { launchWaslSession, removeSession } from "../../bridge/api";
import type { GraphNode, GraphView } from "../../bridge/workstreamTypes";
import { GraphScreen } from "./GraphScreen";
import { isPaused } from "../../bridge/sessionStatus";
import { validateConnection, CONN_ERROR_MESSAGE } from "../../bridge/connValidate";
import { orderDraftLaunch } from "../../bridge/draftOrder";
import { peersForNode } from "../../bridge/peersForNode";
import { buildWaslTargetKey } from "../../bridge/waslTargetKey";

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
    removeDraftNode,
    removeDraftEdge,
    removeConnection,
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

  // Design-time connection-validation error (Phase C). Surfaced when
  // validateConnection rejects a draft wire, or when addConnection rejects for
  // a real edge. Cleared on next successful connection attempt.
  const [designError, setDesignError] = useState<string | null>(null);

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
        bi: e.bi,
        status: "normal" as const,
        has_gate: false,
      })),
    ],
  };

  // Design mode when the store still has a draft in flight, when the backend
  // graph is empty, or when the user explicitly toggled in.
  // Draft nodes ALWAYS force design mode regardless of the seeded value, because
  // the test can inject draftNodes into a graph that already has real nodes (the
  // mixed draft↔real scenario) and the confirm-draft UI must be visible.
  const effectiveDesignMode =
    draftNodes.length > 0 || (designMode ?? realGraph.nodes.length === 0);

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

  // #5 — clicking a node in design mode. Only DRAFT (unlaunched) nodes are
  // modifiable (decision C): open the modal prefilled ("Modify node"). Real nodes
  // are already launched — clicking them is a no-op here (their edit path is the
  // live session, not the draft form).
  function onModifyNode(id: string): void {
    if (!draftTempIds.has(id)) return;
    openNodeModal({
      wsId: effectiveWsId,
      wsName: realGraph.name,
      isFirstNode: false,
      designMode: true,
      modifyTempId: id,
    });
  }

  // Drag-to-connect result. In design mode, a wire whose endpoint is a DRAFT
  // (temp) node cannot be a real connection (its endpoints aren't launched
  // sessions yet — core-ext.md:69-72); record a draft edge instead. A wire
  // between two REAL nodes goes straight to the backend as before.
  // Phase C: validate draft wires via validateConnection; surface errors via
  // designError state. Fix the silent `void` discard for real edges: catch and
  // surface rejection as a designError.
  function onConnect(from: string, to: string, bi: boolean): void {
    if (effectiveDesignMode && (draftTempIds.has(from) || draftTempIds.has(to))) {
      // Validate over the merged draft+real graph.
      const mergedNodes = g.nodes.map((n) => ({ id: n.id, ws: effectiveWsId }));
      const mergedEdges = g.edges.map((e) => ({ from: e.from, to: e.to }));
      const errorKind = validateConnection(effectiveWsId, from, to, mergedNodes, mergedEdges);
      if (errorKind != null) {
        setDesignError(CONN_ERROR_MESSAGE[errorKind]);
        return;
      }
      setDesignError(null);
      addDraftEdge({ from, to, bi });
      return;
    }
    // Real edge: fix the silent void — catch rejections and surface as designError.
    addConnection(genConnId(), effectiveWsId, from, to, bi)
      .then(() => { setDesignError(null); })
      .catch((err: unknown) => {
        const msg = err instanceof Error ? err.message : String(err);
        setDesignError(msg);
      });
  }

  // Change an EXISTING connection's direction (Directed ⇄ Consult). A draft edge
  // is re-authored client-side (remove + re-add with the new `bi`, preserving its
  // endpoints); a real edge is remove+re-add against the backend (there is no
  // in-place mutate on core — the peer topology is derived from the edge set, so
  // a clean replace keeps registration consistent).
  function onSetConnectionBi(id: string, bi: boolean): void {
    if (draftEdgeIds.has(id)) {
      const edge = draftEdges.find((e) => e.tempId === id);
      if (!edge) return;
      removeDraftEdge(id);
      addDraftEdge({ from: edge.from, to: edge.to, bi });
      return;
    }
    const edge = realGraph.edges.find((e) => e.id === id);
    if (!edge) return;
    void removeConnection(id).then(() =>
      addConnection(genConnId(), effectiveWsId, edge.from, edge.to, bi)
    );
  }

  // "Confirm & launch" — the ONE backend materialization point. Launch every
  // draft node in DEPENDENCY ORDER (via orderDraftLaunch — lead first at depth
  // 0, then dependents), assign each with the REAL id returned by launch,
  // building a temp→real map; then submit each draft edge with the resolved
  // real endpoint ids. Finally clear the draft + leave design mode and refresh
  // the graph so the now-real nodes render.
  // A "Build it out" workstream runs EVERY agent over the Wasl transport (#8,
  // decision A) — the picked driver is the NATIVE protocol, launched via Wasl
  // behind the scenes (the user only ever sees native names). So this path
  // always launchWaslSession, protocol = node.driver. Uses orderDraftLaunch for
  // ordering; on failure surface designError and do NOT clearDraft.
  async function onConfirmDraft(): Promise<void> {
    if (draftNodes.length === 0) {
      // No draft authored — just leave design mode (operate-mode "done").
      setDesignMode(false);
      return;
    }

    // Sort by dependency depth (lead first at depth 0, dependents after).
    const ordered = orderDraftLaunch(draftNodes, draftEdges);
    // Feature A: host_id -> display label map (from the host registry) so each
    // peer entry (and the self node) can carry its USER-facing host name, not
    // just the opaque host id.
    const hostLabel = new Map<string, string>(
      (state.snapshot?.hosts ?? []).map((h) => [h.id, h.label])
    );
    const tempToReal = new Map<string, string>();
    try {
      for (const node of ordered) {
        // The native driver ("claude"/"codex"/"opencode") IS the Wasl protocol.
        const protocol = node.driver;
        // Build peers: outbound edges from this node (own from-edges + reverse
        // of consult edges — peersForNode honours `bi`), mapped to
        // {agent_id, host_id, name, host_name}. node.tempId and the peer agent_id
        // share the same draft-node id namespace — CRITICAL consistency rule.
        // Feature A: also carry the peer's display name + host display name so the
        // inner agent addresses peers by the names a human gave them.
        const outboundIds = peersForNode(node.tempId, draftEdges).outbound;
        const peers = outboundIds.flatMap((peerId) => {
          const peerNode = draftNodes.find((n) => n.tempId === peerId);
          if (!peerNode) return []; // skip unresolvable peer
          return [
            {
              agent_id: peerId,
              host_id: peerNode.hostId,
              name: peerNode.name,
              host_name: hostLabel.get(peerNode.hostId) ?? "",
            },
          ];
        });
        // W2b: thread resume state captured at draft-authoring time.
        const wasl_target_key = buildWaslTargetKey(
          node.tempId,
          protocol,
          node.project,
          node.model,
          node.session_mode,
          peers,
          node.resume ?? false,
          node.selectedResume ?? "continue",
          node.resumeSessions ?? [],
          // Feature A: the self node's host display name.
          hostLabel.get(node.hostId) ?? ""
        );
        const realId = await launchWaslSession({
          host_id: node.hostId,
          wasl_target_key,
          name: node.name,
        });
        await assignSession(realId, effectiveWsId, node.isLead);
        tempToReal.set(node.tempId, realId);
      }
      for (const edge of draftEdges) {
        const from = tempToReal.get(edge.from) ?? edge.from;
        const to = tempToReal.get(edge.to) ?? edge.to;
        await addConnection(genConnId(), effectiveWsId, from, to, edge.bi);
      }
      clearDraft();
      setDesignMode(false);
      setDesignError(null);
      // Refresh the graph so the freshly-launched real nodes replace the drafts.
      await goToGraph(effectiveWsId);
    } catch (err) {
      // Surface error — do NOT clearDraft so the user can retry.
      const msg = err instanceof Error ? err.message : String(err);
      setDesignError(msg);
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

  // Remove an authored node. A DRAFT (temp) node is discarded client-side (it
  // never became a real session). A REAL node deletes its session (confirmed) —
  // marsad-core cascades the workstream away if it was the last node (#5), so we
  // fall back to the board when the graph empties out.
  const draftEdgeIds = new Set(draftEdges.map((e) => e.tempId));
  function onRemoveNode(id: string): void {
    if (draftTempIds.has(id)) {
      removeDraftNode(id);
      return;
    }
    const node = realGraph.nodes.find((n) => n.id === id);
    const label = node?.name || id;
    const ok =
      typeof window === "undefined" || typeof window.confirm !== "function"
        ? true
        : window.confirm(`Remove agent "${label}"? This deletes its session and cannot be undone.`);
    if (!ok) return;
    const wasLastReal = realGraph.nodes.length === 1;
    void removeSession(id).then(() => {
      // Last node → the workstream is gone (core cascade); return to the board.
      if (wasLastReal) openBoard();
      else void goToGraph(effectiveWsId);
    });
  }

  // Remove a connection. A DRAFT edge is discarded client-side; a REAL edge hits
  // the backend (removeConnection refreshes the graph itself).
  function onRemoveConnection(id: string): void {
    if (draftEdgeIds.has(id)) {
      removeDraftEdge(id);
      return;
    }
    void removeConnection(id);
  }

  // BUG #6 — the Pause-all / Resume-all affordances must reflect the workstream's
  // ACTUAL session states, not both show unconditionally. A node is paused/stopped
  // when it isn't live (authoritative `SessionView.live` — the SAME signal the
  // dashboard card uses via sessionStatus.isPaused). When a node has no matching
  // SessionView (e.g. a draft, or before its view lands) we fall back to its graph
  // Phase, where a restored/stopped session surfaces as Stale/Detached.
  // Show Pause-all only if at least one node is running; Resume-all only if at
  // least one is paused; both only when mixed.
  const sessions = state.snapshot?.sessions ?? [];
  const nodePaused = (n: GraphNode): boolean => {
    const sv = sessions.find((s) => s.id === n.id);
    if (sv) return isPaused(sv);
    return n.state === "Stale" || n.state === "Detached";
  };
  const realNodes = realGraph.nodes;
  const canPause = realNodes.some((n) => !nodePaused(n));
  const canResume = realNodes.some((n) => nodePaused(n));

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
      onSetConnectionBi={onSetConnectionBi}
      onModifyNode={onModifyNode}
      onRemoveNode={onRemoveNode}
      isDraftNode={(id) => draftTempIds.has(id)}
      onRemoveConnection={onRemoveConnection}
      onPauseWorkstream={canPause ? () => void pauseWorkstream(effectiveWsId) : undefined}
      onResumeWorkstream={canResume ? () => void resumeWorkstream(effectiveWsId) : undefined}
      onDeleteWorkstream={onDeleteWorkstream}
      designError={designError}
    />
  );
}
