/**
 * Board screen — Level 1 "Workstreams" board.
 *
 * Pure/presentational: takes `board: BoardView` and `onOpenWorkstream` as
 * props. No `useVm`, no Tauri, no network — see docs/specs/ui-board.md.
 */
import { Fragment } from "react";
import type { CSSProperties } from "react";
import type {
  BoardView,
  Driver,
  HeadState,
  HostChip,
  LinkChip,
  StateCounts,
  WorkstreamCard,
} from "../bridge/workstreamTypes";

// ---------------------------------------------------------------------------
// State colour / label helpers (mirrors design tokens; Level-1 head states)
// ---------------------------------------------------------------------------

const headStateColor: Record<HeadState, string> = {
  needs: "var(--needs)",
  error: "var(--error)",
  working: "var(--accent)",
  done: "var(--done)",
};

const headStateBg: Record<HeadState, string> = {
  needs: "var(--needs-bg)",
  error: "var(--error-bg)",
  working: "var(--working-bg)",
  done: "var(--done-bg)",
};

const driverColor: Record<Driver, string> = {
  claude: "#d97757",
  codex: "#3fb6a8",
  gemini: "#9b8cf0",
};

const hostStatusColor: Record<string, string> = {
  connected: "var(--done)",
  connecting: "var(--needs)",
  reconnecting: "var(--needs)",
  unreachable: "var(--error)",
  down: "var(--error)",
};

function pulsesFor(state: HeadState | null): boolean {
  return state === "working" || state === "needs";
}

/** Host-kind glyph (design §Iconography): local `▪`, WSL `⊞`, remote `⤓`. */
function hostIcon(kind: string): string {
  if (kind === "wsl") return "⊞";
  if (kind === "remote") return "⤓";
  return "▪";
}

/** Default contextual CTA label when the data source doesn't supply one. */
function defaultCta(state: HeadState | null): string {
  if (state === "needs") return "Review";
  return "Open";
}

/** Non-zero status buckets from `counts`, in a fixed priority order. */
function nonZeroBuckets(counts: StateCounts): { key: keyof StateCounts; label: string }[] {
  const buckets: { key: keyof StateCounts; label: string }[] = [
    { key: "working", label: "working" },
    { key: "needs", label: "needs you" },
    { key: "error", label: "error" },
    { key: "done", label: "done" },
  ];
  return buckets.filter((b) => counts[b.key] > 0);
}

// ---------------------------------------------------------------------------
// State dot (pulses for working/needs)
// ---------------------------------------------------------------------------

function HeadStateDot({ state, size = 9 }: { state: HeadState | null; size?: number }) {
  const pulse = pulsesFor(state);
  const color = state ? headStateColor[state] : "var(--neutral)";
  return (
    <span
      data-pulse={pulse ? "true" : "false"}
      style={{
        width: size,
        height: size,
        borderRadius: "50%",
        background: color,
        display: "inline-block",
        flexShrink: 0,
        animation: pulse ? "dotpulse 1.4s infinite" : undefined,
      }}
    />
  );
}

// ---------------------------------------------------------------------------
// Driver dot cluster
// ---------------------------------------------------------------------------

function DriverDots({ drivers }: { drivers: Driver[] }) {
  return (
    <>
      {drivers.map((d, i) => (
        <span
          key={`${d}-${i}`}
          data-testid="driver-dot"
          data-driver={d}
          title={d}
          style={{
            width: 7,
            height: 7,
            borderRadius: "50%",
            background: driverColor[d],
            display: "inline-block",
            flexShrink: 0,
          }}
        />
      ))}
    </>
  );
}

// ---------------------------------------------------------------------------
// Host chip cluster (mono "▪ label" pills)
// ---------------------------------------------------------------------------

function HostChipPills({
  hostIds,
  hostChips,
  variant = "pill",
}: {
  hostIds: string[];
  hostChips: HostChip[];
  /** "pill" — panel2 background pill (workstream rows). "plain" — bare mono
   * text, no background (attention-card header). */
  variant?: "pill" | "plain";
}) {
  return (
    <>
      {hostIds.map((id) => {
        const host = hostChips.find((h) => h.id === id);
        const label = host?.label ?? id;
        const icon = hostIcon(host?.kind ?? "local");
        const pillStyle: CSSProperties =
          variant === "pill"
            ? {
                color: "var(--txt2)",
                background: "var(--panel2)",
                border: "1px solid var(--border)",
                borderRadius: 5,
                padding: "2px 7px",
              }
            : { color: "var(--txt3)" };
        return (
          <span
            key={id}
            className="mono"
            style={{
              fontSize: "calc(9.5px * var(--fs,1))",
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
              ...pillStyle,
            }}
          >
            {icon} {label}
          </span>
        );
      })}
    </>
  );
}

// ---------------------------------------------------------------------------
// Host health strip
// ---------------------------------------------------------------------------

function hostStatusLabel(status: string): string {
  if (status === "unreachable") return "unreachable";
  return status;
}

function HostStrip({ board }: { board: BoardView }) {
  return (
    <div
      data-testid="host-strip"
      style={{
        display: "flex",
        flexWrap: "wrap",
        alignItems: "center",
        gap: 9,
        margin: "0 0 22px",
      }}
    >
      <span
        style={{
          fontSize: "calc(10px * var(--fs,1))",
          fontWeight: 700,
          letterSpacing: 0.6,
          textTransform: "uppercase",
          color: "var(--txt3)",
          marginRight: 2,
        }}
      >
        Hosts
      </span>
      {board.host_chips.map((host) => {
        const color = hostStatusColor[host.status] ?? "var(--txt3)";
        const pulse = host.status === "reconnecting" || host.status === "unreachable" || host.status === "down";
        return (
          <span
            key={host.id}
            data-testid={`host-chip-${host.id}`}
            data-status={host.status}
            style={{
              display: "inline-flex",
              alignItems: "center",
              gap: 7,
              padding: "5px 11px",
              borderRadius: 8,
              background: "var(--panel)",
              border: `1px solid ${host.status === "connected" ? "var(--border)" : color}`,
            }}
          >
            <span
              data-pulse={pulse ? "true" : "false"}
              style={{
                width: 8,
                height: 8,
                borderRadius: "50%",
                background: color,
                display: "inline-block",
                flexShrink: 0,
                animation: pulse ? `dotpulse ${pulse && host.status === "reconnecting" ? "1s" : "0.8s"} infinite` : undefined,
              }}
            />
            <span className="mono" style={{ fontSize: "calc(11px * var(--fs,1))", color: "var(--txt2)" }}>
              {hostIcon(host.kind)} {host.label}
            </span>
            <span style={{ fontSize: "calc(10px * var(--fs,1))", fontWeight: 600, color }}>
              {hostStatusLabel(host.status)}
            </span>
          </span>
        );
      })}
      {board.link_chips.map((link) => (
        <LinkChipPill key={`${link.a}-${link.b}`} link={link} hostChips={board.host_chips} />
      ))}
    </div>
  );
}

function LinkChipPill({ link, hostChips }: { link: LinkChip; hostChips: HostChip[] }) {
  const labelA = hostChips.find((h) => h.id === link.a)?.label ?? link.a;
  const labelB = hostChips.find((h) => h.id === link.b)?.label ?? link.b;
  return (
    <span
      data-testid={`link-chip-${link.a}-${link.b}`}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 7,
        padding: "5px 11px",
        borderRadius: 8,
        background: "var(--needs-bg)",
        border: "1px solid var(--needs)",
        color: "var(--needs)",
      }}
    >
      <span style={{ fontSize: "calc(11px * var(--fs,1))" }}>⚠</span>
      <span className="mono" style={{ fontSize: "calc(11px * var(--fs,1))" }}>
        {labelA} ⇄ {labelB} link {link.status}
      </span>
      {link.note && (
        <span style={{ fontSize: "calc(10px * var(--fs,1))", color: "var(--txt2)" }}>{link.note}</span>
      )}
    </span>
  );
}

// ---------------------------------------------------------------------------
// Attention card
// ---------------------------------------------------------------------------

function AttentionCard({
  card,
  hostChips,
  onOpen,
}: {
  card: WorkstreamCard;
  hostChips: HostChip[];
  onOpen: () => void;
}) {
  const state = card.head_state;
  const color = state ? headStateColor[state] : "var(--border)";
  const bg = state ? headStateBg[state] : "transparent";
  const headline = card.headline ?? card.goal;
  const cta = card.cta_label ?? defaultCta(state);

  return (
    <div
      data-testid={`attention-card-${card.id}`}
      onClick={onOpen}
      style={{
        borderRadius: 14,
        padding: 16,
        background: "var(--panel)",
        border: `1px solid ${color}`,
        boxShadow: `0 0 22px ${bg}`,
        position: "relative",
        overflow: "hidden",
        cursor: "pointer",
      }}
    >
      <div
        style={{
          position: "absolute",
          left: 0,
          top: 0,
          bottom: 0,
          width: 3,
          background: color,
        }}
      />
      <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 5 }}>
        <span
          data-head-state={state ?? undefined}
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: 6,
            fontSize: "calc(9px * var(--fs,1))",
            fontWeight: 700,
            letterSpacing: 0.4,
            padding: "2px 7px",
            borderRadius: 5,
            color,
            background: bg,
            textTransform: "uppercase",
          }}
        >
          <HeadStateDot state={state} size={7} />
          {state ?? "unknown"}
        </span>
        <div style={{ flex: 1 }} />
        <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
          <HostChipPills hostIds={card.host_chips} hostChips={hostChips} variant="plain" />
        </div>
      </div>
      <div
        style={{
          fontWeight: 700,
          fontSize: "calc(15.5px * var(--fs,1))",
          letterSpacing: -0.2,
        }}
      >
        {card.name}
      </div>
      <div
        style={{
          fontSize: "calc(11.5px * var(--fs,1))",
          color: "var(--txt3)",
          marginTop: 2,
        }}
      >
        {card.goal}
      </div>
      <div
        data-testid={`attention-headline-${card.id}`}
        style={{
          margin: "12px 0",
          padding: "10px 12px",
          borderRadius: 9,
          background: "var(--panel2)",
          border: "1px solid var(--border)",
          fontSize: "calc(12px * var(--fs,1))",
          color: "var(--txt)",
        }}
      >
        {headline}
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: 9 }}>
        <button
          data-testid={`attention-cta-${card.id}`}
          style={{
            flex: 1,
            fontFamily: "inherit",
            fontWeight: 700,
            fontSize: "calc(12.5px * var(--fs,1))",
            padding: 9,
            borderRadius: 9,
            border: "none",
            background: color,
            color: "#0e1116",
            cursor: "pointer",
          }}
        >
          {cta}
        </button>
        <div style={{ display: "flex", alignItems: "center", gap: 5 }}>
          <SessionDots counts={card.counts} />
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Session status dots (attention card footer) — one 9px dot per session,
// tinted by that session's state, working ones pulsing (design §Attention
// card footer). Derived from `counts` (board-level data has no per-session
// list) in a fixed priority order: working, needs, error, done.
// ---------------------------------------------------------------------------

function SessionDots({ counts }: { counts: StateCounts }) {
  const order: (keyof StateCounts)[] = ["working", "needs", "error", "done"];
  const dots: { key: keyof StateCounts; i: number }[] = [];
  for (const key of order) {
    for (let i = 0; i < counts[key]; i++) dots.push({ key, i });
  }
  return (
    <>
      {dots.map((d) => (
        <span
          key={`${d.key}-${d.i}`}
          data-testid="session-dot"
          data-session-state={d.key}
          title={d.key}
          style={{
            width: 9,
            height: 9,
            borderRadius: "50%",
            background: headStateColor[d.key as HeadState],
            display: "inline-block",
            flexShrink: 0,
            animation: d.key === "working" ? "dotpulse 1.3s infinite" : undefined,
          }}
        />
      ))}
    </>
  );
}

// ---------------------------------------------------------------------------
// Workstream row (all-section)
// ---------------------------------------------------------------------------

function WsActionBtn({
  testid,
  label,
  danger,
  onClick,
}: {
  testid: string;
  label: string;
  danger?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      data-testid={testid}
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
      style={{
        fontFamily: "inherit",
        fontSize: "calc(11px * var(--fs,1))",
        fontWeight: 600,
        padding: "4px 9px",
        borderRadius: 7,
        cursor: "pointer",
        border: danger ? "1px solid var(--error)" : "1px solid var(--border2)",
        background: danger ? "var(--error-bg)" : "var(--panel2)",
        color: danger ? "var(--error)" : "var(--txt2)",
        whiteSpace: "nowrap",
        flexShrink: 0,
      }}
    >
      {label}
    </button>
  );
}

function WorkstreamRow({
  card,
  hostChips,
  onOpen,
  onPause,
  onResume,
  onDelete,
}: {
  card: WorkstreamCard;
  hostChips: HostChip[];
  onOpen: () => void;
  onPause: () => void;
  onResume: () => void;
  onDelete: () => void;
}) {
  const state = card.head_state;
  const color = state ? headStateColor[state] : "var(--border)";
  const bg = state ? headStateBg[state] : "transparent";
  const buckets = nonZeroBuckets(card.counts);

  return (
    <div
      data-testid={`workstream-row-${card.id}`}
      onClick={onOpen}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 16,
        padding: "14px 16px",
        borderRadius: 13,
        background: "var(--panel)",
        border: "1px solid var(--border)",
        cursor: "pointer",
      }}
    >
      <HeadStateDot state={state} size={10} />

      <div style={{ minWidth: 210, maxWidth: 280 }}>
        <div
          style={{
            fontWeight: 600,
            fontSize: "calc(13.5px * var(--fs,1))",
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {card.name}
        </div>
        <div
          style={{
            fontSize: "calc(11px * var(--fs,1))",
            color: "var(--txt3)",
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {card.goal}
        </div>
      </div>

      <div style={{ width: 150, display: "flex", alignItems: "center", gap: 6, flexShrink: 0 }}>
        <HostChipPills hostIds={card.host_chips} hostChips={hostChips} variant="pill" />
      </div>

      <div style={{ width: 88, display: "flex", alignItems: "center", gap: 5, flexShrink: 0 }}>
        <DriverDots drivers={Array.from(new Set(card.driver_dots))} />
        <span
          className="mono"
          style={{ fontSize: "calc(10.5px * var(--fs,1))", color: "var(--txt3)", marginLeft: 4 }}
        >
          {card.session_count} sessions
        </span>
      </div>

      <div style={{ flex: 1, display: "flex", alignItems: "center", flexWrap: "wrap", gap: 6 }}>
        {buckets.map((b) => (
          <span
            key={b.key}
            style={{
              fontSize: "calc(10px * var(--fs,1))",
              fontWeight: 600,
              padding: "2px 8px",
              borderRadius: 6,
              color: headStateColor[b.key as HeadState] ?? "var(--txt2)",
              background: headStateBg[b.key as HeadState] ?? "var(--panel2)",
            }}
          >
            {card.counts[b.key]} {b.label}
          </span>
        ))}
      </div>

      <span
        data-head-state={state ?? undefined}
        style={{
          fontSize: "calc(9px * var(--fs,1))",
          fontWeight: 700,
          letterSpacing: 0.3,
          padding: "3px 9px",
          borderRadius: 6,
          color,
          background: bg,
          textTransform: "uppercase",
          flexShrink: 0,
        }}
      >
        {state ?? "—"}
      </span>

      {/* Workstream lifecycle actions — state-appropriate: running → Pause;
          stopped/idle → Resume. Delete always visible. Each stops row-click
          propagation so it doesn't also open the workstream. */}
      <div style={{ display: "flex", alignItems: "center", gap: 6, flexShrink: 0 }}>
        {(state === "working" || state === "needs" || state === "error") ? (
          <WsActionBtn testid={`ws-pause-${card.id}`} label="⏸ Pause" onClick={onPause} />
        ) : (
          <WsActionBtn testid={`ws-resume-${card.id}`} label="▶ Resume" onClick={onResume} />
        )}
        <WsActionBtn testid={`ws-delete-${card.id}`} label="✕ Delete" danger onClick={onDelete} />
      </div>

      <span style={{ color: "var(--txt3)", fontSize: "calc(15px * var(--fs,1))", flexShrink: 0 }}>›</span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Board — main export
// ---------------------------------------------------------------------------

export interface BoardProps {
  board: BoardView;
  onOpenWorkstream: (id: string) => void;
  /** Pause every session in a workstream (#5). Optional — presentational
   *  render sites (tests) may omit lifecycle wiring. */
  onPauseWorkstream?: (id: string) => void;
  /** Resume every session in a workstream (#5). */
  onResumeWorkstream?: (id: string) => void;
  /** Delete a workstream (kills all its sessions) — confirmed by the caller (#5). */
  onDeleteWorkstream?: (id: string) => void;
}

export function Board({
  board,
  onOpenWorkstream,
  onPauseWorkstream,
  onResumeWorkstream,
  onDeleteWorkstream,
}: BoardProps) {
  const { summary } = board;

  // Group the dashboard into Live / Done / Paused so active work sits on top.
  // Live = working/needs/error; Done = finished; Paused = null head_state (every
  // node Stale/Detached). Stable order preserved within each group.
  const liveCards = board.all.filter(
    (c) => c.head_state === "working" || c.head_state === "needs" || c.head_state === "error"
  );
  const doneCards = board.all.filter((c) => c.head_state === "done");
  const pausedCards = board.all.filter((c) => c.head_state === null);

  return (
    <div
      data-testid="board-root"
      style={{
        padding: "22px 26px 40px",
        maxWidth: 1200,
        margin: "0 auto",
        color: "var(--txt)",
      }}
    >
      {/* Title row */}
      <div style={{ display: "flex", alignItems: "baseline", gap: 12, marginBottom: 3 }}>
        <h1
          data-testid="board-title"
          style={{
            fontSize: "calc(21px * var(--fs,1))",
            fontWeight: 700,
            margin: 0,
            letterSpacing: -0.3,
          }}
        >
          Workstreams
        </h1>
        <span
          data-testid="board-summary"
          style={{ fontSize: "calc(12.5px * var(--fs,1))", color: "var(--txt2)" }}
        >
          {summary.workstreams} workstreams · {summary.sessions} sessions · {summary.hosts} hosts
        </span>
      </div>

      <p style={{ fontSize: "calc(12.5px * var(--fs,1))", color: "var(--txt3)", margin: "0 0 14px" }}>
        A workstream is one or more sessions across one or more hosts. Open one to inspect its
        graph, or jump straight into a session.
      </p>

      {/* Host health strip */}
      <HostStrip board={board} />

      {/* Needs your attention */}
      {board.attention.length > 0 && (
        <>
          <div style={{ display: "flex", alignItems: "center", gap: 9, margin: "0 0 12px" }}>
            <span
              style={{
                width: 9,
                height: 9,
                borderRadius: "50%",
                background: "var(--needs)",
                animation: "dotpulse 1.4s infinite",
                display: "inline-block",
                flexShrink: 0,
              }}
            />
            <h2
              style={{
                fontSize: "calc(13px * var(--fs,1))",
                fontWeight: 700,
                letterSpacing: 0.4,
                textTransform: "uppercase",
                margin: 0,
                color: "var(--needs)",
              }}
            >
              Needs your attention
            </h2>
          </div>
          <div
            data-testid="attention-section"
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fill, minmax(380px, 1fr))",
              gap: 13,
              marginBottom: 26,
            }}
          >
            {board.attention.map((card) => (
              <AttentionCard
                key={card.id}
                card={card}
                hostChips={board.host_chips}
                onOpen={() => onOpenWorkstream(card.id)}
              />
            ))}
          </div>
        </>
      )}

      {/* All workstreams */}
      <h2
        style={{
          fontSize: "calc(13px * var(--fs,1))",
          fontWeight: 700,
          letterSpacing: 0.4,
          textTransform: "uppercase",
          margin: "0 0 12px",
          color: "var(--txt2)",
        }}
      >
        All workstreams
      </h2>
      <div data-testid="all-section" style={{ display: "flex", flexDirection: "column", gap: 9 }}>
        {([
          { key: "live", label: null, cards: liveCards },
          { key: "done", label: "Done", cards: doneCards },
          { key: "paused", label: "Paused", cards: pausedCards },
        ] as const)
          .filter((g) => g.cards.length > 0)
          .map((g, gi) => (
            <Fragment key={g.key}>
              {g.label && (
                <div
                  data-testid={`${g.key}-divider`}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 9,
                    margin: gi > 0 ? "10px 0 3px" : "0 0 3px",
                    fontSize: "calc(10.5px * var(--fs,1))",
                    fontWeight: 700,
                    letterSpacing: 0.5,
                    textTransform: "uppercase",
                    color: "var(--txt3)",
                  }}
                >
                  {g.label}
                  <div style={{ flex: 1, height: 1, background: "var(--border)" }} />
                </div>
              )}
              {g.cards.map((card) => (
                <WorkstreamRow
                  key={card.id}
                  card={card}
                  hostChips={board.host_chips}
                  onOpen={() => onOpenWorkstream(card.id)}
                  onPause={() => onPauseWorkstream?.(card.id)}
                  onResume={() => onResumeWorkstream?.(card.id)}
                  onDelete={() => onDeleteWorkstream?.(card.id)}
                />
              ))}
            </Fragment>
          ))}
      </div>
    </div>
  );
}
