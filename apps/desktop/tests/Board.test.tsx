/**
 * Behavioral tests for the Board screen (Level 1 — Workstreams Board).
 *
 * Board is pure/presentational: it takes `board: BoardView` and
 * `onOpenWorkstream: (id: string) => void` as props — no VmProvider, no
 * Tauri runtime needed. Mirrors the render/query/fireEvent style of
 * tests/Overview.test.tsx, wrapped in ThemeProvider for token access.
 *
 * Covers spec §Test plan UB1–UB9 from docs/specs/ui-board.md.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { Board } from "../src/screens/Board";
import { MOCK_BOARD } from "../src/bridge/mockBoard";
import type { BoardView, WorkstreamCard, HostChip, LinkChip } from "../src/bridge/workstreamTypes";

// ---------------------------------------------------------------------------
// Helper: render Board wrapped in ThemeProvider
// ---------------------------------------------------------------------------

function renderBoard(board: BoardView = MOCK_BOARD, onOpenWorkstream = vi.fn()) {
  const utils = render(
    <ThemeProvider initialTheme="dark">
      <Board board={board} onOpenWorkstream={onOpenWorkstream} />
    </ThemeProvider>
  );
  return { ...utils, onOpenWorkstream };
}

// ---------------------------------------------------------------------------
// Fixture builders — small inline BoardView objects for cases that need
// specific data rather than relying only on MOCK_BOARD.
// ---------------------------------------------------------------------------

function makeCard(overrides: Partial<WorkstreamCard> = {}): WorkstreamCard {
  return {
    id: "w-test",
    name: "test-workstream",
    goal: "test goal",
    head_state: "working",
    driver_dots: ["claude"],
    host_chips: ["localhost"],
    session_count: 1,
    counts: { working: 1, needs: 0, error: 0, done: 0 },
    needs_attention: false,
    is_single_node: true,
    ...overrides,
  };
}

function makeHostChip(overrides: Partial<HostChip> = {}): HostChip {
  return {
    id: "host-a",
    label: "localhost",
    kind: "local",
    status: "connected",
    ...overrides,
  };
}

function makeBoard(overrides: Partial<BoardView> = {}): BoardView {
  return {
    summary: { workstreams: 1, sessions: 1, hosts: 1 },
    host_chips: [makeHostChip()],
    link_chips: [],
    attention: [],
    all: [makeCard()],
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// Single source of truth: dashboard derives per-session status from the SAME
// SessionView the detail view uses (via card.node_ids → state.sessions), not the
// phase-aggregate head_state. Regressions for the dashboard/detail drift.
// ---------------------------------------------------------------------------

import type { SessionView } from "../src/bridge/types";

function makeSession(overrides: Partial<SessionView> = {}): SessionView {
  return {
    id: "s1",
    host_id: "host-a",
    state: "Done",
    live: true,
    conn_health: "Connected",
    ...overrides,
  } as unknown as SessionView;
}

describe("Board — single source of truth (SessionView-driven status)", () => {
  it("shows PAUSE (not Resume) for a Done-but-live session — matches the detail view", () => {
    // Bug 3: head_state='done' used to force a Resume button even though the
    // session is live (running). Now it reads liveness from the SessionView.
    const card = makeCard({
      id: "w-done-live",
      head_state: "done",
      node_ids: ["sess-done-live"],
      counts: { working: 0, needs: 0, error: 0, done: 1 },
    });
    const board = makeBoard({ all: [card] });
    const session = makeSession({ id: "sess-done-live", state: "Done", live: true });
    render(
      <ThemeProvider initialTheme="dark">
        <Board board={board} sessions={[session]} onOpenWorkstream={vi.fn()} />
      </ThemeProvider>
    );
    expect(screen.getByTestId("ws-pause-w-done-live")).toBeTruthy();
    expect(screen.queryByTestId("ws-resume-w-done-live")).toBeNull();
  });

  it("shows RESUME for a Done-and-paused session", () => {
    const card = makeCard({
      id: "w-done-paused",
      head_state: "done",
      node_ids: ["sess-done-paused"],
      counts: { working: 0, needs: 0, error: 0, done: 1 },
    });
    const board = makeBoard({ all: [card] });
    const session = makeSession({ id: "sess-done-paused", state: "Done", live: false });
    render(
      <ThemeProvider initialTheme="dark">
        <Board board={board} sessions={[session]} onOpenWorkstream={vi.fn()} />
      </ThemeProvider>
    );
    expect(screen.getByTestId("ws-resume-w-done-paused")).toBeTruthy();
    expect(screen.queryByTestId("ws-pause-w-done-paused")).toBeNull();
  });

  it("surfaces the per-session 'Unstable' connection hint on the dashboard card", () => {
    // Bug 1: conn_health=Unstable showed in the detail view but never on the board.
    const card = makeCard({
      id: "w-unstable",
      head_state: "working",
      node_ids: ["sess-unstable"],
    });
    const board = makeBoard({ all: [card] });
    const session = makeSession({
      id: "sess-unstable",
      state: "Working",
      live: true,
      conn_health: "Unstable",
    });
    render(
      <ThemeProvider initialTheme="dark">
        <Board board={board} sessions={[session]} onOpenWorkstream={vi.fn()} />
      </ThemeProvider>
    );
    const chip = screen.getByTestId("ws-conn-w-unstable");
    expect(chip.textContent).toContain("Unstable");
  });

  it("shows the Unstable hint on an attention card too", () => {
    const card = makeCard({
      id: "w-attn",
      head_state: "needs",
      needs_attention: true,
      node_ids: ["sess-attn"],
      counts: { working: 0, needs: 1, error: 0, done: 0 },
    });
    const board = makeBoard({ all: [card], attention: [card] });
    const session = makeSession({
      id: "sess-attn",
      state: "Needs",
      live: true,
      conn_health: "Unstable",
    });
    render(
      <ThemeProvider initialTheme="dark">
        <Board board={board} sessions={[session]} onOpenWorkstream={vi.fn()} />
      </ThemeProvider>
    );
    expect(screen.getByTestId("attention-conn-w-attn").textContent).toContain("Unstable");
  });
});

// ---------------------------------------------------------------------------
// UB1 — summary line
// ---------------------------------------------------------------------------

describe("Board — UB1 summary line", () => {
  it("shows the workstream/session/host counts from MOCK_BOARD.summary", () => {
    renderBoard();
    const summary = screen.getByTestId("board-summary");
    const { workstreams, sessions, hosts } = MOCK_BOARD.summary;
    expect(summary.textContent).toContain(`${workstreams} workstreams`);
    expect(summary.textContent).toContain(`${sessions} sessions`);
    expect(summary.textContent).toContain(`${hosts} hosts`);
  });

  it("reflects a custom summary object exactly (not hardcoded)", () => {
    const board = makeBoard({ summary: { workstreams: 7, sessions: 42, hosts: 5 } });
    renderBoard(board);
    const summary = screen.getByTestId("board-summary");
    expect(summary.textContent).toContain("7 workstreams");
    expect(summary.textContent).toContain("42 sessions");
    expect(summary.textContent).toContain("5 hosts");
  });
});

// ---------------------------------------------------------------------------
// UB2 — host chips
// ---------------------------------------------------------------------------

describe("Board — UB2 host chips", () => {
  it("renders one host chip per host_chips entry, each showing its label", () => {
    renderBoard();
    for (const host of MOCK_BOARD.host_chips) {
      const chip = screen.getByTestId(`host-chip-${host.id}`);
      expect(chip).toBeTruthy();
      expect(chip.textContent).toContain(host.label);
    }
  });

  it("a connected host chip carries data-status=\"connected\"", () => {
    const connected = MOCK_BOARD.host_chips.find((h) => h.status === "connected");
    expect(connected).toBeTruthy();
    renderBoard();
    const chip = screen.getByTestId(`host-chip-${connected!.id}`);
    expect(chip.getAttribute("data-status")).toBe("connected");
  });

  it("a reconnecting host chip carries data-status=\"reconnecting\"", () => {
    const reconnecting = MOCK_BOARD.host_chips.find((h) => h.status === "reconnecting");
    expect(reconnecting).toBeTruthy();
    renderBoard();
    const chip = screen.getByTestId(`host-chip-${reconnecting!.id}`);
    expect(chip.getAttribute("data-status")).toBe("reconnecting");
  });

  it("renders exactly the number of host chips present in host_chips (no extras)", () => {
    const board = makeBoard({
      host_chips: [
        makeHostChip({ id: "h1", label: "localhost", status: "connected" }),
        makeHostChip({ id: "h2", label: "wsl-box", status: "reconnecting" }),
      ],
    });
    renderBoard(board);
    expect(screen.getByTestId("host-chip-h1")).toBeTruthy();
    expect(screen.getByTestId("host-chip-h2")).toBeTruthy();
    expect(screen.queryByTestId("host-chip-h3")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// UB3 — link chips
// ---------------------------------------------------------------------------

describe("Board — UB3 link chips", () => {
  it("renders a link-chip-{a}-{b} for a degraded link, and no chip for a fine pair", () => {
    const board = makeBoard({
      host_chips: [
        makeHostChip({ id: "hostA", label: "hostA", status: "connected" }),
        makeHostChip({ id: "hostB", label: "hostB", status: "connected" }),
        makeHostChip({ id: "hostC", label: "hostC", status: "connected" }),
      ],
      link_chips: [
        { a: "hostA", b: "hostB", status: "degraded", note: "packet loss · 38%" } as LinkChip,
      ],
    });
    renderBoard(board);
    const chip = screen.getByTestId("link-chip-hostA-hostB");
    expect(chip).toBeTruthy();
    expect(chip.textContent).toContain("hostA");
    expect(chip.textContent).toContain("hostB");
    // No link chip for the hostA-hostC or hostB-hostC pairs — they have no
    // degraded/down entry in link_chips.
    expect(screen.queryByTestId("link-chip-hostA-hostC")).toBeNull();
    expect(screen.queryByTestId("link-chip-hostB-hostC")).toBeNull();
  });

  it("renders only the degraded/down link chips present in MOCK_BOARD, matching count exactly", () => {
    renderBoard();
    for (const link of MOCK_BOARD.link_chips) {
      expect(screen.getByTestId(`link-chip-${link.a}-${link.b}`)).toBeTruthy();
    }
  });

  it("renders no link chips at all when link_chips is empty", () => {
    const board = makeBoard({
      host_chips: [
        makeHostChip({ id: "hostA", label: "hostA" }),
        makeHostChip({ id: "hostB", label: "hostB" }),
      ],
      link_chips: [],
    });
    const { container } = renderBoard(board);
    expect(container.querySelectorAll('[data-testid^="link-chip-"]').length).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// UB4 — attention section content
// ---------------------------------------------------------------------------

describe("Board — UB4 attention section content", () => {
  it("renders exactly the attention cards from MOCK_BOARD by testid", () => {
    renderBoard();
    expect(MOCK_BOARD.attention.length).toBeGreaterThan(0);
    for (const card of MOCK_BOARD.attention) {
      expect(screen.getByTestId(`attention-card-${card.id}`)).toBeTruthy();
    }
  });

  it("each attention card shows the workstream name and goal", () => {
    renderBoard();
    for (const card of MOCK_BOARD.attention) {
      const el = screen.getByTestId(`attention-card-${card.id}`);
      expect(el.textContent).toContain(card.name);
      expect(el.textContent).toContain(card.goal);
    }
  });

  it("a needs-state attention card has data-head-state=\"needs\"", () => {
    const needsCard = MOCK_BOARD.attention.find((c) => c.head_state === "needs");
    expect(needsCard).toBeTruthy();
    renderBoard();
    const el = screen.getByTestId(`attention-card-${needsCard!.id}`);
    const pill = el.querySelector('[data-head-state]');
    expect(pill).toBeTruthy();
    expect(pill!.getAttribute("data-head-state")).toBe("needs");
  });

  it("an error-state attention card has data-head-state=\"error\"", () => {
    const errorCard = MOCK_BOARD.attention.find((c) => c.head_state === "error");
    expect(errorCard).toBeTruthy();
    renderBoard();
    const el = screen.getByTestId(`attention-card-${errorCard!.id}`);
    const pill = el.querySelector('[data-head-state]');
    expect(pill).toBeTruthy();
    expect(pill!.getAttribute("data-head-state")).toBe("error");
  });
});

// ---------------------------------------------------------------------------
// UB5 — attention section absent when empty
// ---------------------------------------------------------------------------

describe("Board — UB5 attention section empty state", () => {
  it("does not render attention-section when board.attention is empty", () => {
    const board = makeBoard({ attention: [] });
    renderBoard(board);
    expect(screen.queryByTestId("attention-section")).toBeNull();
  });

  it("renders attention-section when board.attention has at least one card", () => {
    const board = makeBoard({
      attention: [makeCard({ id: "w-needs", head_state: "needs", needs_attention: true })],
    });
    renderBoard(board);
    expect(screen.getByTestId("attention-section")).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// UB6 — all-section rows: session_count + non-zero status pills
// ---------------------------------------------------------------------------

describe("Board — UB6 all-section rows", () => {
  it("renders one workstream-row per all card", () => {
    renderBoard();
    for (const card of MOCK_BOARD.all) {
      expect(screen.getByTestId(`workstream-row-${card.id}`)).toBeTruthy();
    }
  });

  it("all-section is always rendered even alongside an attention section", () => {
    renderBoard();
    expect(screen.getByTestId("all-section")).toBeTruthy();
  });

  it("a row shows its session_count as 'N sessions'", () => {
    const board = makeBoard({
      all: [makeCard({ id: "w1", session_count: 4 })],
    });
    renderBoard(board);
    const row = screen.getByTestId("workstream-row-w1");
    expect(row.textContent).toContain("4 sessions");
  });

  it("a row shows only the non-zero status pills from counts", () => {
    const board = makeBoard({
      all: [
        makeCard({
          id: "w2",
          session_count: 4,
          counts: { working: 3, needs: 1, error: 0, done: 0 },
        }),
      ],
    });
    renderBoard(board);
    const row = screen.getByTestId("workstream-row-w2");
    expect(row.textContent).toContain("3 working");
    expect(row.textContent).toMatch(/1 needs you/i);
    // error and done are zero — must not render a "0 ..." pill
    expect(row.textContent).not.toMatch(/0 error/i);
    expect(row.textContent).not.toMatch(/0 done/i);
    expect(row.textContent).not.toMatch(/0 working/i);
  });

  it("a row with all-done counts shows only a 'done' pill and no other buckets", () => {
    const board = makeBoard({
      all: [
        makeCard({
          id: "w3",
          session_count: 2,
          head_state: "done",
          counts: { working: 0, needs: 0, error: 0, done: 2 },
        }),
      ],
    });
    renderBoard(board);
    const row = screen.getByTestId("workstream-row-w3");
    expect(row.textContent).toContain("2 done");
    expect(row.textContent).not.toMatch(/0 working/i);
    expect(row.textContent).not.toMatch(/0 needs/i);
    expect(row.textContent).not.toMatch(/0 error/i);
  });
});

// ---------------------------------------------------------------------------
// UB7 — driver dots
// ---------------------------------------------------------------------------

describe("Board — UB7 driver dots", () => {
  it("a multi-driver card renders one driver-dot per distinct driver, in order", () => {
    const board = makeBoard({
      all: [
        makeCard({
          id: "w-multi",
          driver_dots: ["claude", "codex", "gemini"],
        }),
      ],
    });
    renderBoard(board);
    const row = screen.getByTestId("workstream-row-w-multi");
    const dots = Array.from(row.querySelectorAll('[data-testid="driver-dot"]'));
    expect(dots.length).toBe(3);
    expect(dots.map((d) => d.getAttribute("data-driver"))).toEqual([
      "claude",
      "codex",
      "gemini",
    ]);
  });

  it("a single-driver card renders exactly one driver-dot with the right data-driver", () => {
    const board = makeBoard({
      all: [makeCard({ id: "w-single", driver_dots: ["codex"] })],
    });
    renderBoard(board);
    const row = screen.getByTestId("workstream-row-w-single");
    const dots = Array.from(row.querySelectorAll('[data-testid="driver-dot"]'));
    expect(dots.length).toBe(1);
    expect(dots[0].getAttribute("data-driver")).toBe("codex");
  });

  it("MOCK_BOARD's multi-driver card (if present) renders a driver-dot per distinct driver", () => {
    const multiDriverCard = MOCK_BOARD.all.find((c) => c.driver_dots.length > 1);
    expect(multiDriverCard).toBeTruthy();
    renderBoard();
    const row = screen.getByTestId(`workstream-row-${multiDriverCard!.id}`);
    const dots = Array.from(row.querySelectorAll('[data-testid="driver-dot"]'));
    expect(dots.map((d) => d.getAttribute("data-driver"))).toEqual(
      multiDriverCard!.driver_dots
    );
  });
});

// ---------------------------------------------------------------------------
// UB8 — click interactions call onOpenWorkstream
// ---------------------------------------------------------------------------

describe("Board — UB8 click interactions", () => {
  it("clicking an attention card calls onOpenWorkstream with that id", () => {
    const board = makeBoard({
      attention: [makeCard({ id: "w-attn-click", head_state: "needs", needs_attention: true })],
    });
    const onOpenWorkstream = vi.fn();
    renderBoard(board, onOpenWorkstream);
    fireEvent.click(screen.getByTestId("attention-card-w-attn-click"));
    expect(onOpenWorkstream).toHaveBeenCalledWith("w-attn-click");
    expect(onOpenWorkstream).toHaveBeenCalledTimes(1);
  });

  it("clicking an all-row calls onOpenWorkstream with that id", () => {
    const board = makeBoard({
      all: [makeCard({ id: "w-row-click" })],
    });
    const onOpenWorkstream = vi.fn();
    renderBoard(board, onOpenWorkstream);
    fireEvent.click(screen.getByTestId("workstream-row-w-row-click"));
    expect(onOpenWorkstream).toHaveBeenCalledWith("w-row-click");
    expect(onOpenWorkstream).toHaveBeenCalledTimes(1);
  });

  it("clicking one attention card does not fire onOpenWorkstream for a different id", () => {
    const board = makeBoard({
      attention: [
        makeCard({ id: "w-a", head_state: "needs", needs_attention: true }),
        makeCard({ id: "w-b", head_state: "error", needs_attention: true }),
      ],
    });
    const onOpenWorkstream = vi.fn();
    renderBoard(board, onOpenWorkstream);
    fireEvent.click(screen.getByTestId("attention-card-w-a"));
    expect(onOpenWorkstream).toHaveBeenCalledWith("w-a");
    expect(onOpenWorkstream).not.toHaveBeenCalledWith("w-b");
  });

  it("clicking real MOCK_BOARD attention and all-row entries dispatches correct ids", () => {
    const onOpenWorkstream = vi.fn();
    renderBoard(MOCK_BOARD, onOpenWorkstream);

    const firstAttention = MOCK_BOARD.attention[0];
    fireEvent.click(screen.getByTestId(`attention-card-${firstAttention.id}`));
    expect(onOpenWorkstream).toHaveBeenCalledWith(firstAttention.id);

    const firstRow = MOCK_BOARD.all[0];
    fireEvent.click(screen.getByTestId(`workstream-row-${firstRow.id}`));
    expect(onOpenWorkstream).toHaveBeenCalledWith(firstRow.id);
  });
});

// ---------------------------------------------------------------------------
// UB9 — pulsing state dot for working/needs, not for done
// ---------------------------------------------------------------------------

describe("Board — UB9 pulsing state dot", () => {
  it("a working-state all-row marks its state dot as data-pulse=\"true\"", () => {
    const board = makeBoard({
      all: [makeCard({ id: "w-working", head_state: "working" })],
    });
    renderBoard(board);
    const row = screen.getByTestId("workstream-row-w-working");
    const dot = row.querySelector('[data-pulse]');
    expect(dot).toBeTruthy();
    expect(dot!.getAttribute("data-pulse")).toBe("true");
  });

  it("a needs-state all-row marks its state dot as data-pulse=\"true\"", () => {
    const board = makeBoard({
      all: [makeCard({ id: "w-needs-row", head_state: "needs", needs_attention: true })],
    });
    renderBoard(board);
    const row = screen.getByTestId("workstream-row-w-needs-row");
    const dot = row.querySelector('[data-pulse]');
    expect(dot).toBeTruthy();
    expect(dot!.getAttribute("data-pulse")).toBe("true");
  });

  it("a done-state all-row does NOT mark its state dot as pulsing", () => {
    const board = makeBoard({
      all: [
        makeCard({
          id: "w-done",
          head_state: "done",
          counts: { working: 0, needs: 0, error: 0, done: 1 },
        }),
      ],
    });
    renderBoard(board);
    const row = screen.getByTestId("workstream-row-w-done");
    const dot = row.querySelector('[data-pulse]');
    // Either no data-pulse attribute at all, or explicitly "false" — never "true".
    if (dot) {
      expect(dot.getAttribute("data-pulse")).not.toBe("true");
    } else {
      expect(dot).toBeNull();
    }
  });

  it("a needs-state attention card marks its state dot as pulsing", () => {
    const board = makeBoard({
      attention: [makeCard({ id: "w-attn-needs", head_state: "needs", needs_attention: true })],
    });
    renderBoard(board);
    const card = screen.getByTestId("attention-card-w-attn-needs");
    const dot = card.querySelector('[data-pulse]');
    expect(dot).toBeTruthy();
    expect(dot!.getAttribute("data-pulse")).toBe("true");
  });
});

// ---------------------------------------------------------------------------
// UB10 — centering (design §Level 1 Board layout: max content width 1200px,
// centered, padding 22px 26px 40px)
// ---------------------------------------------------------------------------

describe("Board — UB10 centering", () => {
  it("the board root is centered at max-width 1200px with the exact screen padding", () => {
    renderBoard();
    const root = screen.getByTestId("board-root");
    expect(root.style.maxWidth).toBe("1200px");
    expect(root.style.margin).toBe("0px auto");
    expect(root.style.padding).toBe("22px 26px 40px");
  });
});

// ---------------------------------------------------------------------------
// UB11 — link-issue chips resolve host labels (not raw ids) and surface the
// note verbatim (design §Host health strip: "hostA ⇄ hostB link degraded" +
// a muted note, e.g. "packet loss · 38%").
// ---------------------------------------------------------------------------

describe("Board — UB11 link-issue chip content", () => {
  it("resolves host_chips labels for a and b instead of the raw host ids", () => {
    const board = makeBoard({
      host_chips: [
        makeHostChip({ id: "h-a", label: "hetzner", status: "connected" }),
        makeHostChip({ id: "h-b", label: "gpu-box", status: "reconnecting" }),
      ],
      link_chips: [{ a: "h-a", b: "h-b", status: "degraded", note: "packet loss · 38%" }],
    });
    renderBoard(board);
    const chip = screen.getByTestId("link-chip-h-a-h-b");
    expect(chip.textContent).toContain("hetzner");
    expect(chip.textContent).toContain("gpu-box");
    expect(chip.textContent).toContain("link degraded");
    expect(chip.textContent).toContain("packet loss · 38%");
    // The raw ids themselves must not leak into the rendered text.
    expect(chip.textContent).not.toContain("h-a ⇄");
  });

  it("MOCK_BOARD's link chip carries its note text", () => {
    renderBoard();
    const link = MOCK_BOARD.link_chips[0];
    const chip = screen.getByTestId(`link-chip-${link.a}-${link.b}`);
    expect(chip.textContent).toContain(link.note ?? "");
  });
});

// ---------------------------------------------------------------------------
// UB12 — head-state priority (needs > error > working > done) is trusted
// and faithfully rendered by the head-state pill, regardless of which
// bucket in `counts` has the largest count.
// ---------------------------------------------------------------------------

describe("Board — UB12 head-state priority reflection", () => {
  it.each([
    { head_state: "needs" as const, counts: { working: 5, needs: 1, error: 2, done: 3 } },
    { head_state: "error" as const, counts: { working: 5, needs: 0, error: 1, done: 3 } },
    { head_state: "working" as const, counts: { working: 1, needs: 0, error: 0, done: 5 } },
    { head_state: "done" as const, counts: { working: 0, needs: 0, error: 0, done: 5 } },
  ])(
    "a row surfaces head_state=$head_state as its pill even when other buckets outnumber it",
    ({ head_state, counts }) => {
      const board = makeBoard({
        all: [makeCard({ id: "w-priority", head_state, counts })],
      });
      renderBoard(board);
      const row = screen.getByTestId("workstream-row-w-priority");
      const pill = row.querySelector(`[data-head-state="${head_state}"]`);
      expect(pill).toBeTruthy();
      expect(pill!.textContent?.toLowerCase()).toContain(head_state);
    },
  );

  it("an attention card with both needs and error sessions present shows needs (highest priority)", () => {
    const board = makeBoard({
      attention: [
        makeCard({
          id: "w-needs-over-error",
          head_state: "needs",
          counts: { working: 0, needs: 1, error: 1, done: 0 },
          needs_attention: true,
        }),
      ],
    });
    renderBoard(board);
    const card = screen.getByTestId("attention-card-w-needs-over-error");
    expect(card.querySelector('[data-head-state="needs"]')).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// UB13 — attention card headline + contextual CTA (design §Attention card:
// headline info box + a CTA labelled "Review handoff" / "Review" / "Open").
// ---------------------------------------------------------------------------

describe("Board — UB13 attention card headline + CTA", () => {
  it("renders the card's headline verbatim when supplied", () => {
    const board = makeBoard({
      attention: [
        makeCard({
          id: "w-headline",
          head_state: "needs",
          headline: "api-auth-refactor wants to run: npx prisma migrate deploy",
          needs_attention: true,
        }),
      ],
    });
    renderBoard(board);
    const headline = screen.getByTestId("attention-headline-w-headline");
    expect(headline.textContent).toBe("api-auth-refactor wants to run: npx prisma migrate deploy");
  });

  it("falls back to the workstream goal when headline is absent", () => {
    const board = makeBoard({
      attention: [
        makeCard({ id: "w-no-headline", head_state: "needs", goal: "test goal", needs_attention: true }),
      ],
    });
    renderBoard(board);
    const headline = screen.getByTestId("attention-headline-w-no-headline");
    expect(headline.textContent).toBe("test goal");
  });

  it("uses the supplied cta_label verbatim ('Review handoff')", () => {
    const board = makeBoard({
      attention: [
        makeCard({
          id: "w-cta",
          head_state: "error",
          cta_label: "Review handoff",
          needs_attention: true,
        }),
      ],
    });
    renderBoard(board);
    expect(screen.getByTestId("attention-cta-w-cta").textContent).toBe("Review handoff");
  });

  it("falls back to a head-state-derived CTA ('Review' for needs, 'Open' otherwise) when absent", () => {
    const board = makeBoard({
      attention: [
        makeCard({ id: "w-cta-needs", head_state: "needs", needs_attention: true }),
        makeCard({ id: "w-cta-error", head_state: "error", needs_attention: true }),
      ],
    });
    renderBoard(board);
    expect(screen.getByTestId("attention-cta-w-cta-needs").textContent).toBe("Review");
    expect(screen.getByTestId("attention-cta-w-cta-error").textContent).toBe("Open");
  });

  it("MOCK_BOARD's attention cards carry a headline and cta_label", () => {
    renderBoard();
    for (const card of MOCK_BOARD.attention) {
      expect(card.headline).toBeTruthy();
      expect(card.cta_label).toBeTruthy();
      expect(screen.getByTestId(`attention-headline-${card.id}`).textContent).toBe(card.headline);
      expect(screen.getByTestId(`attention-cta-${card.id}`).textContent).toBe(card.cta_label);
    }
  });
});

// ---------------------------------------------------------------------------
// UB14 — attention card session dots: one dot per session (from `counts`),
// working ones pulsing.
// ---------------------------------------------------------------------------

describe("Board — UB14 attention card session dots", () => {
  it("renders one session-dot per session in `counts`, total matching session_count", () => {
    const board = makeBoard({
      attention: [
        makeCard({
          id: "w-dots",
          head_state: "needs",
          session_count: 4,
          counts: { working: 2, needs: 1, error: 0, done: 1 },
          needs_attention: true,
        }),
      ],
    });
    renderBoard(board);
    const card = screen.getByTestId("attention-card-w-dots");
    const dots = card.querySelectorAll('[data-testid="session-dot"]');
    expect(dots.length).toBe(4);
  });

  it("marks working session-dots as pulsing and non-working ones as not", () => {
    const board = makeBoard({
      attention: [
        makeCard({
          id: "w-dots-pulse",
          head_state: "needs",
          session_count: 2,
          counts: { working: 1, needs: 0, error: 0, done: 1 },
          needs_attention: true,
        }),
      ],
    });
    renderBoard(board);
    const card = screen.getByTestId("attention-card-w-dots-pulse");
    const workingDot = card.querySelector('[data-session-state="working"]');
    const doneDot = card.querySelector('[data-session-state="done"]');
    expect(workingDot).toBeTruthy();
    expect(doneDot).toBeTruthy();
    expect((workingDot as HTMLElement).style.animation).toContain("dotpulse");
    expect((doneDot as HTMLElement).style.animation).toBe("");
  });
});
