/**
 * BoardScreen — connected wrapper for the Level-1 workstreams board.
 *
 * Consumes `useVm()` and passes props down to the presentational `Board`.
 * No prop-drilling from App; the wrapper owns the state binding.
 * See docs/specs/ui-app-wiring.md §Connected wrappers.
 */
import { useVm } from "../bridge/useVm";
import { Board } from "./Board";

export function BoardScreen(): JSX.Element {
  const { state, openWorkstream } = useVm();

  if (state.board == null) {
    return <div data-testid="board-loading" />;
  }

  return <Board board={state.board} onOpenWorkstream={openWorkstream} />;
}
