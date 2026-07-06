/**
 * BoardScreen — connected wrapper for the Level-1 workstreams board.
 *
 * Consumes `useVm()` and passes props down to the presentational `Board`.
 * No prop-drilling from App; the wrapper owns the state binding.
 * See docs/specs/ui-app-wiring.md §Connected wrappers.
 */
import { useMemo } from "react";
import { useVm } from "../bridge/useVm";
import type { Host } from "../bridge/types";
import type { BoardView, HostChip } from "../bridge/workstreamTypes";
import { Board } from "./Board";

/** Map a live `Host` (snapshot) → the Board's `HostChip` shape. The Board
 *  host-health strip reads lowercase `status` and a `kind` glyph key, while the
 *  snapshot host carries capitalized `HostStatus` and `is_local`/`target`. */
function hostToChip(h: Host): HostChip {
  const kind = h.is_local
    ? "local"
    : h.target?.startsWith("wsl:")
    ? "wsl"
    : "remote";
  return {
    id: h.id,
    label: h.label,
    kind,
    status: h.status.toLowerCase(),
  };
}

export function BoardScreen(): JSX.Element {
  const { state, openWorkstream, pauseWorkstream, resumeWorkstream, deleteWorkstream } = useVm();

  // BUG #1 — the board's host chips are fetched ONCE via fetchBoard(); live
  // connection changes arrive as `host_health`/`hosts` vm-updates into
  // `state.snapshot.hosts`. Derive the host-health strip from that live source
  // so status updates render without navigating away and back. Falls back to
  // the static `board.host_chips` only when the snapshot has no hosts yet.
  const liveHostChips = useMemo<HostChip[] | null>(() => {
    const hosts = state.snapshot?.hosts ?? [];
    if (hosts.length === 0) return null;
    return hosts.map(hostToChip);
  }, [state.snapshot?.hosts]);

  const board: BoardView | null = useMemo(() => {
    if (state.board == null) return null;
    if (liveHostChips == null) return state.board;
    return { ...state.board, host_chips: liveHostChips };
  }, [state.board, liveHostChips]);

  if (board == null) {
    return <div data-testid="board-loading" />;
  }

  function handleDelete(id: string) {
    const card = board?.all.find((c) => c.id === id);
    const label = card?.name ?? id;
    const ok =
      typeof window === "undefined" || typeof window.confirm !== "function"
        ? true
        : window.confirm(
            `Delete workstream "${label}"? This kills all its sessions and cannot be undone.`
          );
    if (!ok) return;
    void deleteWorkstream(id);
  }

  return (
    <Board
      board={board}
      onOpenWorkstream={openWorkstream}
      onPauseWorkstream={(id) => void pauseWorkstream(id)}
      onResumeWorkstream={(id) => void resumeWorkstream(id)}
      onDeleteWorkstream={handleDelete}
    />
  );
}
