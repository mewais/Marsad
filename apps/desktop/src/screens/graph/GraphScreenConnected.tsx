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
import { useEffect, useRef, useState } from "react";
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
  const {
    state,
    selectNode,
    openNode,
    addConnection,
    openNodeModal,
    pauseWorkstream,
    resumeWorkstream,
    deleteWorkstream,
    openBoard,
  } = useVm();
  // #3/#4 — design-mode entry (THE launch unblocker). A workstream that is
  // NEW or has ZERO sessions must open in DESIGN mode so "＋ Add agent" is
  // shown (in operate mode an empty workstream only shows "✎ Update workflow"
  // and there's no way to add — nothing could ever be launched). Once a
  // workstream has sessions it opens in operate mode; the user can still flip
  // back to design via "✎ Update workflow".
  //
  // `designMode` is null until we've seeded it for the currently-open
  // workstream; we re-seed whenever the open workstream id changes (opening a
  // different one) so each workstream gets the right initial mode, while an
  // explicit user toggle within a workstream is preserved.
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
  const g = graph; // non-null binding for the closures below

  const effectiveDesignMode = designMode ?? g.nodes.length === 0;
  const effectiveWsId = wsId ?? g.ws_id;

  // Interim gate wiring: route the gate affordance to the RECEIVING node's
  // session (the dedicated handoff-approval modal is a later block). This keeps
  // the control LIVE and testable — never a dead handler.
  function onOpenGate(connId: string): void {
    const edge = g.edges.find((x) => x.id === connId);
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
    // First agent in an empty workstream ⇒ isFirstNode so Launch routes
    // straight to Level-3 and the node becomes the workstream's lead.
    openNodeModal({
      wsId: effectiveWsId,
      wsName: g.name,
      isFirstNode: g.nodes.length === 0,
    });
  }

  function onConnect(from: string, to: string, kind: Parameters<typeof addConnection>[4], bi: boolean): void {
    void addConnection(genConnId(), effectiveWsId, from, to, kind, bi);
  }

  // "Discard" in the design panel. For a fresh, empty "Build it out" draft
  // (0 nodes / no session yet) this means abandoning the whole workstream —
  // route back to the Board, where `openBoard` discards the pending draft so
  // no orphan empty workstream lingers. For a workstream that already has
  // nodes/sessions, Discard just leaves design mode (never deletes real work).
  function onDiscardDraft(): void {
    if (g.nodes.length === 0 && state.draftWsId === effectiveWsId) {
      openBoard();
    } else {
      setDesignMode(false);
    }
  }

  function onDeleteWorkstream(): void {
    const label = g.name || effectiveWsId;
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
      selectedNode={state.nodeSel}
      onSelectNode={selectNode}
      onOpenNode={openNode}
      onOpenGate={onOpenGate}
      designMode={effectiveDesignMode}
      onEnterDesign={() => setDesignMode(true)}
      onConfirmDraft={() => setDesignMode(false)}
      onDiscardDraft={onDiscardDraft}
      onAddAgent={onAddAgent}
      onConnect={onConnect}
      onPauseWorkstream={() => void pauseWorkstream(effectiveWsId)}
      onResumeWorkstream={() => void resumeWorkstream(effectiveWsId)}
      onDeleteWorkstream={onDeleteWorkstream}
    />
  );
}
