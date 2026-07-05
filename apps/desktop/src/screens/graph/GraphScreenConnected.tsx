/**
 * GraphScreenConnected — connected wrapper for the Level-2 workstream graph.
 *
 * Consumes `useVm()` and passes props down to the presentational `GraphScreen`.
 * No prop-drilling from App; the wrapper owns the state binding — INCLUDING
 * design-mode authoring, which is ephemeral client-side UI state (not part of
 * `VmState`): a workstream's nodes/edges are already real backend data the
 * moment its graph is fetched, so "design mode" here means "show the
 * authoring chrome (connect handles, Design panel) over the live graph" —
 * `onConnect` still calls the REAL `addConnection` command helper immediately
 * (nothing is queued client-side to fake-launch later).
 *
 * See docs/specs/ui-app-wiring.md §Connected wrappers.
 */
import { useState } from "react";
import { useVm } from "../../bridge/useVm";
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

export function GraphScreenConnected(): JSX.Element {
  const { state, selectNode, openNode, addConnection, openNodeModal } = useVm();
  const [designMode, setDesignMode] = useState(false);

  if (state.graph == null) {
    return <div data-testid="graph-loading" />;
  }

  const graph = state.graph;
  const wsId = state.wsId ?? graph.ws_id;

  // Interim gate wiring: route the gate affordance to the RECEIVING node's
  // session (the dedicated handoff-approval modal is a later block). This keeps
  // the control LIVE and testable — never a dead handler.
  function onOpenGate(connId: string): void {
    const edge = graph.edges.find((x) => x.id === connId);
    if (edge) {
      openNode(edge.to);
    }
  }

  // "＋ Add agent" — opens the node/add-agent modal (owned by the modals
  // slice: driver/host/wiring pickers, "Controlled by"/"Feeds from" to wire
  // the new node to existing ones). `isFirstNode: false` since this is always
  // reached from an EXISTING workstream's graph (never the empty "Build it
  // out" canvas, which is the New-workstream wizard's concern).
  function onAddAgent(): void {
    openNodeModal({ wsId, wsName: graph.name, isFirstNode: false });
  }

  function onConnect(from: string, to: string, kind: Parameters<typeof addConnection>[4], bi: boolean): void {
    void addConnection(genConnId(), wsId, from, to, kind, bi);
  }

  return (
    <GraphScreen
      graph={graph}
      selectedNode={state.nodeSel}
      onSelectNode={selectNode}
      onOpenNode={openNode}
      onOpenGate={onOpenGate}
      designMode={designMode}
      onEnterDesign={() => setDesignMode(true)}
      onConfirmDraft={() => setDesignMode(false)}
      onDiscardDraft={() => setDesignMode(false)}
      onAddAgent={onAddAgent}
      onConnect={onConnect}
    />
  );
}
