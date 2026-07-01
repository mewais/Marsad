/**
 * GraphScreenConnected — connected wrapper for the Level-2 workstream graph.
 *
 * Consumes `useVm()` and passes props down to the presentational `GraphScreen`.
 * No prop-drilling from App; the wrapper owns the state binding.
 * See docs/specs/ui-app-wiring.md §Connected wrappers.
 */
import { useVm } from "../../bridge/useVm";
import { GraphScreen } from "./GraphScreen";

export function GraphScreenConnected(): JSX.Element {
  const { state, selectNode, openNode } = useVm();

  if (state.graph == null) {
    return <div data-testid="graph-loading" />;
  }

  const graph = state.graph;

  // Interim gate wiring: route the gate affordance to the RECEIVING node's
  // session (the dedicated handoff-approval modal is a later block). This keeps
  // the control LIVE and testable — never a dead handler.
  function onOpenGate(connId: string): void {
    const edge = graph.edges.find((x) => x.id === connId);
    if (edge) {
      openNode(edge.to);
    }
  }

  return (
    <GraphScreen
      graph={graph}
      selectedNode={state.nodeSel}
      onSelectNode={selectNode}
      onOpenNode={openNode}
      onOpenGate={onOpenGate}
    />
  );
}
