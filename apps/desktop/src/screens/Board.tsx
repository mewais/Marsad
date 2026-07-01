/**
 * Board screen — Level 1 "Workstreams" board.
 *
 * Pure/presentational: takes `board: BoardView` and `onOpenWorkstream` as
 * props. No `useVm`, no Tauri, no network — see docs/specs/ui-board.md.
 */
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

function HostChipPills({ hostIds, hostChips }: { hostIds: string[]; hostChips: HostChip[] }) {
  return (
    <>
      {hostIds.map((id) => {
        const host = hostChips.find((h) => h.id === id);
        const label = host?.label ?? id;
        return (
          <span
            key={id}
            className="mono"
            style={{
              fontSize: "calc(11.5px * var(--fs,1))",
              color: "var(--txt2)",
              background: "var(--panel2)",
              border: "1px solid var(--border)",
              borderRadius: 6,
              padding: "2px 7px",
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
            }}
          >
            ▪ {label}
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
        margin: "14px 0 20px",
      }}
    >
      <span
        style={{
          fontSize: "calc(11.5px * var(--fs,1))",
          fontWeight: 700,
          letterSpacing: 0.4,
          textTransform: "uppercase",
          color: "var(--txt3)",
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
            className="mono"
            style={{
              display: "inline-flex",
              alignItems: "center",
              gap: 6,
              padding: "5px 11px",
              borderRadius: 8,
              background: "var(--panel)",
              border: `1px solid ${host.status === "connected" ? "var(--border)" : color}`,
              fontSize: "calc(12.5px * var(--fs,1))",
              color: "var(--txt2)",
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
                animation: pulse ? "dotpulse 1s infinite" : undefined,
              }}
            />
            {host.label}
            <span style={{ color: "var(--txt3)" }}>{hostStatusLabel(host.status)}</span>
          </span>
        );
      })}
      {board.link_chips.map((link) => (
        <LinkChipPill key={`${link.a}-${link.b}`} link={link} />
      ))}
    </div>
  );
}

function LinkChipPill({ link }: { link: LinkChip }) {
  return (
    <span
      data-testid={`link-chip-${link.a}-${link.b}`}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
        padding: "5px 11px",
        borderRadius: 8,
        background: "var(--needs-bg)",
        border: "1px solid var(--needs)",
        fontSize: "calc(12.5px * var(--fs,1))",
        color: "var(--needs)",
      }}
    >
      <span>⚠</span>
      <span className="mono">
        {link.a} ⇄ {link.b} link {link.status}
      </span>
      {link.note && <span style={{ color: "var(--txt3)" }}>({link.note})</span>}
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

  return (
    <div
      data-testid={`attention-card-${card.id}`}
      onClick={onOpen}
      style={{
        borderRadius: 14,
        padding: 16,
        background: "var(--panel)",
        border: `1px solid ${color}`,
        boxShadow: state === "needs" || state === "error" ? `0 0 22px ${bg}` : undefined,
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
      <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8 }}>
        <span
          data-head-state={state ?? undefined}
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: 6,
            fontSize: "calc(11.5px * var(--fs,1))",
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
        <HostChipPills hostIds={card.host_chips} hostChips={hostChips} />
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
      <div style={{ display: "flex", alignItems: "center", gap: 6, marginTop: 12 }}>
        <DriverDots drivers={Array.from(new Set(card.driver_dots))} />
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Workstream row (all-section)
// ---------------------------------------------------------------------------

function WorkstreamRow({
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

      <div style={{ width: 150, display: "flex", flexWrap: "wrap", gap: 4 }}>
        <HostChipPills hostIds={card.host_chips} hostChips={hostChips} />
      </div>

      <div style={{ width: 88, display: "flex", alignItems: "center", gap: 4, flexShrink: 0 }}>
        <DriverDots drivers={Array.from(new Set(card.driver_dots))} />
        <span
          className="mono"
          style={{ fontSize: "calc(11.5px * var(--fs,1))", color: "var(--txt3)", marginLeft: 4 }}
        >
          {card.session_count} sessions
        </span>
      </div>

      <div style={{ flex: 1, display: "flex", flexWrap: "wrap", gap: 6 }}>
        {buckets.map((b) => (
          <span
            key={b.key}
            style={{
              fontSize: "calc(11.5px * var(--fs,1))",
              fontWeight: 700,
              padding: "2px 7px",
              borderRadius: 5,
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
          fontSize: "calc(11px * var(--fs,1))",
          fontWeight: 700,
          letterSpacing: 0.4,
          padding: "2px 7px",
          borderRadius: 5,
          color,
          background: bg,
          textTransform: "uppercase",
          flexShrink: 0,
        }}
      >
        {state ?? "—"}
      </span>
      <span style={{ color: "var(--txt3)" }}>›</span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Board — main export
// ---------------------------------------------------------------------------

export interface BoardProps {
  board: BoardView;
  onOpenWorkstream: (id: string) => void;
}

export function Board({ board, onOpenWorkstream }: BoardProps) {
  const { summary } = board;

  return (
    <div
      style={{
        padding: "22px 26px 40px",
        maxWidth: 1200,
        margin: "0 auto",
        color: "var(--txt)",
      }}
    >
      {/* Title row */}
      <div style={{ display: "flex", alignItems: "baseline", gap: 12, marginBottom: 4 }}>
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

      <p style={{ fontSize: "calc(12.5px * var(--fs,1))", color: "var(--txt3)", margin: "4px 0 0" }}>
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
                fontSize: "calc(15.5px * var(--fs,1))",
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
          fontSize: "calc(15.5px * var(--fs,1))",
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
        {board.all.map((card) => (
          <WorkstreamRow
            key={card.id}
            card={card}
            hostChips={board.host_chips}
            onOpen={() => onOpenWorkstream(card.id)}
          />
        ))}
      </div>
    </div>
  );
}
