/**
 * Overview screen — command center with stat tiles, attention gate grid,
 * active rows, and idle/finished grid.
 */
import { useVm } from "../bridge/useVm";
import type { SessionCard } from "../bridge/types";
import { StatTile } from "../components/StatTile";
import { StateDot } from "../components/StateDot";
import { phaseColor, phaseBg, phaseLabel } from "../components/Badge";

// ---------------------------------------------------------------------------
// Gate kind label helpers
// ---------------------------------------------------------------------------

function gateKindLabel(kind: string | null, state?: string): string {
  if (state === "Error") return "Error";
  if (kind === "permission") return "Permission";
  if (kind === "plan") return "Plan";
  if (kind === "edit") return "File change";
  if (kind === "question") return "Question";
  // No gate object at all but the session is Needs — a bare
  // HookEvent::Notification (crates/marsad-events) sets Phase::Needs without
  // ever populating `gate`. Label it distinctly instead of the generic
  // "Gate" fallback so it's clear at a glance this isn't a HITL gate.
  if (state === "Needs") return "Notice";
  return kind ?? "Gate";
}

function gateLead(kind: string | null, state?: string): string {
  if (state === "Error") return "Last error";
  if (kind === "permission") return "Permission request";
  if (kind === "plan") return "Proposed plan";
  if (kind === "edit") return "Wants to start";
  if (kind === "question") return "Wants your input";
  if (state === "Needs") return "Needs your attention";
  return "Gate";
}

// Fallback summary text for a bare-notification "Needs" session (no gate
// object at all) so the summary box never renders a blank "—" placeholder
// with no explanation of why the human is being asked to look.
const NOTIFICATION_FALLBACK_SUMMARY =
  "Claude sent a notification and is waiting for you to check the session.";

// ---------------------------------------------------------------------------
// Context bar color thresholds
// ---------------------------------------------------------------------------

function ctxColor(pct: number): string {
  if (pct >= 85) return "var(--error)";
  if (pct >= 65) return "var(--needs)";
  return "var(--accent)";
}

// ---------------------------------------------------------------------------
// Attention gate card
// ---------------------------------------------------------------------------

interface AttentionCardProps {
  card: SessionCard;
  onReview: () => void;
  onOpen: () => void;
}

function AttentionCard({ card, onReview, onOpen }: AttentionCardProps) {
  const color = phaseColor[card.state];
  const bg = phaseBg[card.state];
  const kind = card.gate_kind;

  // Glow animation keyframes depend on phase
  const glowStyle =
    card.state === "Needs"
      ? "0 0 0 1px var(--needs), 0 0 14px 2px rgba(241,168,58,0.18)"
      : card.state === "Error"
      ? "0 0 0 1px var(--error), 0 0 14px 2px rgba(224,89,74,0.18)"
      : undefined;

  return (
    <div
      data-testid={`attention-card-${card.id}`}
      style={{
        borderRadius: 14,
        padding: 16,
        background: "var(--panel)",
        border: `1px solid ${color}`,
        boxShadow: glowStyle,
        position: "relative",
        overflow: "hidden",
        animation:
          card.state === "Needs" ? "glow 2.6s ease-in-out infinite" : undefined,
      }}
    >
      {/* Left accent bar */}
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

      {/* Card header */}
      <div style={{ display: "flex", alignItems: "flex-start", gap: 10 }}>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              marginBottom: 5,
            }}
          >
            {/* Gate kind badge */}
            <span
              style={{
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
              {gateKindLabel(kind, card.state)}
            </span>
            <span
              className="mono"
              style={{
                fontSize: "calc(12.5px * var(--fs,1))",
                color: "var(--txt3)",
              }}
            >
              {card.host_label}
            </span>
          </div>
          <div
            style={{
              fontWeight: 700,
              fontSize: "calc(18px * var(--fs,1))",
              letterSpacing: -0.2,
            }}
          >
            {card.name}
          </div>
          <div
            className="mono"
            style={{
              fontSize: "calc(13px * var(--fs,1))",
              color: "var(--txt3)",
              marginTop: 2,
            }}
          >
            {card.project}
          </div>
        </div>
      </div>

      {/* Gate summary box */}
      <div
        style={{
          margin: "12px 0",
          padding: "11px 12px",
          borderRadius: 9,
          background: "var(--panel2)",
          border: "1px solid var(--border)",
        }}
      >
        <div
          style={{
            fontSize: "calc(12.5px * var(--fs,1))",
            fontWeight: 700,
            letterSpacing: 0.4,
            textTransform: "uppercase",
            color: "var(--txt3)",
            marginBottom: 5,
          }}
        >
          {gateLead(kind, card.state)}
        </div>
        <div
          className="mono"
          style={{
            fontSize: "calc(14.5px * var(--fs,1))",
            color: "var(--txt)",
            wordBreak: "break-word",
          }}
        >
          {card.gate_summary ??
            (card.state === "Needs" ? NOTIFICATION_FALLBACK_SUMMARY : "—")}
        </div>
      </div>

      {/* Subagent activity — only shown when this session actually has
          subagents (running and/or completed). Nothing rendered otherwise. */}
      {(card.running_subagents > 0 || card.subagents > 0) && (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 6,
            marginBottom: 12,
            fontSize: "calc(13px * var(--fs,1))",
            color: "var(--txt2)",
          }}
        >
          <span className="mono" style={{ color: "var(--accent2)" }}>⊟</span>
          <span>
            {card.running_subagents > 0
              ? `${card.running_subagents} sub-agent${card.running_subagents > 1 ? "s" : ""} running`
              : "Sub-agents"}
            {card.subagents - card.running_subagents > 0 &&
              ` · ${card.subagents - card.running_subagents} done`}
          </span>
        </div>
      )}

      {/* Action buttons */}
      <div style={{ display: "flex", gap: 8 }}>
        <button
          data-testid={`review-btn-${card.id}`}
          onClick={onReview}
          style={{
            flex: 1,
            fontFamily: "inherit",
            fontWeight: 700,
            fontSize: "calc(15px * var(--fs,1))",
            padding: 9,
            borderRadius: 9,
            border: card.state === "Error" ? "1px solid var(--error)" : "none",
            background: card.state === "Error" ? "transparent" : color,
            color: card.state === "Error" ? "var(--error)" : "#0e1116",
            cursor: "pointer",
          }}
        >
          {card.state === "Error" ? "Open session →" : "Review →"}
        </button>
        <button
          data-testid={`open-btn-${card.id}`}
          onClick={onOpen}
          style={{
            fontFamily: "inherit",
            fontWeight: 600,
            fontSize: "calc(15px * var(--fs,1))",
            padding: "9px 13px",
            borderRadius: 9,
            border: "1px solid var(--border2)",
            background: "transparent",
            color: "var(--txt2)",
            cursor: "pointer",
          }}
        >
          Open
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Active row
// ---------------------------------------------------------------------------

interface ActiveRowProps {
  card: SessionCard;
  onClick: () => void;
}

function ActiveRow({ card, onClick }: ActiveRowProps) {
  return (
    <div
      data-testid={`active-row-${card.id}`}
      onClick={onClick}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 14,
        padding: "13px 15px",
        borderRadius: 12,
        background: "var(--panel)",
        border: "1px solid var(--border)",
        cursor: "pointer",
      }}
    >
      {/* Pulsing dot */}
      <StateDot state={card.state} size={9} />

      {/* Name + host/branch */}
      <div style={{ minWidth: 170, maxWidth: 230 }}>
        <div
          style={{
            fontWeight: 600,
            fontSize: "calc(15.5px * var(--fs,1))",
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {card.name}
        </div>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 6,
            marginTop: 1,
          }}
        >
          <span
            className="mono"
            style={{
              fontSize: "calc(12.5px * var(--fs,1))",
              color: "var(--txt3)",
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
            }}
          >
            {card.host_label} · {card.branch}
          </span>
          {/* Subagent chip — visible whenever this session has subagents
              currently running, so live parallel work isn't hidden. */}
          {card.running_subagents > 0 && (
            <span
              style={{
                fontSize: "calc(11.5px * var(--fs,1))",
                fontWeight: 700,
                color: "var(--accent2)",
                background: "var(--accent-bg)",
                padding: "1px 6px",
                borderRadius: 5,
                whiteSpace: "nowrap",
                flexShrink: 0,
              }}
            >
              ⊟ {card.running_subagents} sub-agent{card.running_subagents > 1 ? "s" : ""} running
            </span>
          )}
        </div>
      </div>

      {/* Live action line */}
      <div
        style={{
          flex: 1,
          minWidth: 0,
          display: "flex",
          alignItems: "center",
          gap: 8,
          color: "var(--txt2)",
          fontSize: "calc(14.5px * var(--fs,1))",
        }}
      >
        <span
          className="mono"
          style={{
            fontSize: "calc(13.5px * var(--fs,1))",
            color: "var(--accent2)",
          }}
        >
          ⚙
        </span>
        <span
          style={{
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {card.last_action ?? "Working…"}
        </span>
        {/* Blinking cursor */}
        <span
          style={{
            width: 4,
            height: 13,
            background: "var(--accent2)",
            display: "inline-block",
            animation: "blink 1s step-end infinite",
            borderRadius: 1,
            flexShrink: 0,
          }}
        />
      </div>

      {/* Context bar */}
      <div style={{ width: 120, flexShrink: 0 }}>
        <div
          style={{
            display: "flex",
            justifyContent: "space-between",
            fontSize: "calc(12.5px * var(--fs,1))",
            color: "var(--txt3)",
            marginBottom: 3,
          }}
        >
          <span>ctx {card.context_pct}%</span>
          <span className="mono">
            {card.tokens_label}
          </span>
        </div>
        <div
          style={{
            height: 5,
            borderRadius: 3,
            background: "var(--border)",
            overflow: "hidden",
          }}
        >
          <div
            style={{
              height: "100%",
              borderRadius: 3,
              background: ctxColor(card.context_pct),
              width: `${Math.min(card.context_pct, 100)}%`,
            }}
          />
        </div>
      </div>

      {/* Elapsed */}
      <span
        className="mono"
        style={{
          fontSize: "calc(13.5px * var(--fs,1))",
          color: "var(--txt3)",
          width: 62,
          textAlign: "right",
          flexShrink: 0,
        }}
      >
        {card.elapsed_label || "—"}
      </span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Idle card
// ---------------------------------------------------------------------------

interface IdleCardProps {
  card: SessionCard;
  onClick: () => void;
}

function IdleCard({ card, onClick }: IdleCardProps) {
  const color = phaseColor[card.state];
  const bg = phaseBg[card.state];
  const label = phaseLabel[card.state];

  return (
    <div
      data-testid={`idle-card-${card.id}`}
      onClick={onClick}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 11,
        padding: "12px 14px",
        borderRadius: 11,
        background: "var(--panel)",
        border: "1px solid var(--border)",
        cursor: "pointer",
      }}
    >
      <span
        style={{
          width: 8,
          height: 8,
          borderRadius: "50%",
          background: color,
          flexShrink: 0,
          display: "inline-block",
        }}
      />
      <div style={{ flex: 1, minWidth: 0 }}>
        <div
          style={{
            fontWeight: 600,
            fontSize: "calc(15px * var(--fs,1))",
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {card.name}
        </div>
        <div
          className="mono"
          style={{
            fontSize: "calc(12.5px * var(--fs,1))",
            color: "var(--txt3)",
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {card.host_label} · {card.branch}
        </div>
      </div>
      <span
        style={{
          fontSize: "calc(11.5px * var(--fs,1))",
          fontWeight: 700,
          padding: "2px 6px",
          borderRadius: 5,
          color,
          background: bg,
          textTransform: "uppercase",
          flexShrink: 0,
        }}
      >
        {label}
      </span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Overview — main export
// ---------------------------------------------------------------------------

export function Overview() {
  const { counts, overviewGroups, dispatch } = useVm();
  const { attention, active, idle } = overviewGroups;

  function openSession(id: string) {
    dispatch({ type: "SELECT_SESSION", id });
  }

  function reviewGate(id: string) {
    // Select the session first so gate screen can read selectedId
    dispatch({ type: "SELECT_SESSION", id });
    dispatch({ type: "SET_SCREEN", screen: "gate" });
  }

  return (
    <div
      style={{
        padding: "22px 26px 40px",
        maxWidth: 1280,
        margin: "0 auto",
        color: "var(--txt)",
      }}
    >
      {/* Page title */}
      <div
        style={{
          display: "flex",
          alignItems: "baseline",
          gap: 12,
          marginBottom: 4,
        }}
      >
        <h1
          style={{
            fontSize: "calc(26px * var(--fs,1))",
            fontWeight: 700,
            margin: 0,
            letterSpacing: -0.3,
          }}
        >
          Command center
        </h1>
        <span style={{ fontSize: "calc(15px * var(--fs,1))", color: "var(--txt2)" }}>
          {counts.total} sessions across {counts.host_count} hosts
        </span>
      </div>

      {/* Clock / live subtitle */}
      <p style={{ fontSize: "calc(15px * var(--fs,1))", color: "var(--txt3)", margin: "0 0 18px" }}>
        {counts.working > 0
          ? `Live · ${counts.working} working · updated just now`
          : "Updated just now"}
      </p>

      {/* ── Stat tiles ── */}
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(4, 1fr)",
          gap: 12,
          marginBottom: 22,
        }}
      >
        {/* Needs you — amber border+bg, value in amber */}
        <StatTile
          label="Needs you"
          value={counts.needs}
          sub="awaiting your decision"
          color="var(--needs)"
          border="var(--needs)"
          bg="var(--needs-bg)"
        />
        {/* Errored — default panel bg; label in error color, value in default txt */}
        <div
          style={{
            padding: "15px 16px",
            borderRadius: 13,
            border: "1px solid var(--border)",
            background: "var(--panel)",
          }}
        >
          <div
            style={{
              fontSize: "calc(13.5px * var(--fs,1))",
              fontWeight: 700,
              letterSpacing: 0.4,
              textTransform: "uppercase",
              color: "var(--error)",
            }}
          >
            Errored
          </div>
          <div
            style={{
              fontSize: "calc(37px * var(--fs,1))",
              fontWeight: 800,
              color: "var(--txt)",
              lineHeight: 1.1,
              marginTop: 4,
            }}
          >
            {counts.errors}
          </div>
          <div style={{ fontSize: "calc(13.5px * var(--fs,1))", color: "var(--txt2)", marginTop: 2 }}>
            need a look
          </div>
        </div>
        {/* Working — default panel bg; label in accent color, value in default txt */}
        <div
          style={{
            padding: "15px 16px",
            borderRadius: 13,
            border: "1px solid var(--border)",
            background: "var(--panel)",
          }}
        >
          <div
            style={{
              fontSize: "calc(13.5px * var(--fs,1))",
              fontWeight: 700,
              letterSpacing: 0.4,
              textTransform: "uppercase",
              color: "var(--accent)",
            }}
          >
            Working
          </div>
          <div
            style={{
              fontSize: "calc(37px * var(--fs,1))",
              fontWeight: 800,
              color: "var(--txt)",
              lineHeight: 1.1,
              marginTop: 4,
            }}
          >
            {counts.working}
          </div>
          <div style={{ fontSize: "calc(13.5px * var(--fs,1))", color: "var(--txt2)", marginTop: 2 }}>
            actively running
          </div>
        </div>
        {/* Cost today — default panel bg; label in txt3, value in default txt */}
        <div
          style={{
            padding: "15px 16px",
            borderRadius: 13,
            border: "1px solid var(--border)",
            background: "var(--panel)",
          }}
        >
          <div
            style={{
              fontSize: "calc(13.5px * var(--fs,1))",
              fontWeight: 700,
              letterSpacing: 0.4,
              textTransform: "uppercase",
              color: "var(--txt3)",
            }}
          >
            Cost today
          </div>
          <div
            data-testid="overview-cost-value"
            style={{
              fontSize: "calc(37px * var(--fs,1))",
              fontWeight: 800,
              color: "var(--txt)",
              lineHeight: 1.1,
              marginTop: 4,
            }}
          >
            {counts.cost_estimated && (
              <span
                style={{ fontSize: "0.4em", fontWeight: 700, color: "var(--txt3)", marginRight: 4 }}
                title="Estimated from the local pricing table — no agent-reported cost has been received yet"
              >
                ~ estimated
              </span>
            )}
            {counts.cost}
          </div>
          <div style={{ fontSize: "calc(13.5px * var(--fs,1))", color: "var(--txt2)", marginTop: 2 }}>
            {counts.tokens} tokens
          </div>
        </div>
      </div>

      {/* ── Needs your attention ── */}
      {attention.length > 0 && (
        <>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 9,
              margin: "0 0 12px",
            }}
          >
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
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fill, minmax(360px, 1fr))",
              gap: 13,
              marginBottom: 26,
            }}
          >
            {attention.map((card) => (
              <AttentionCard
                key={card.id}
                card={card}
                onReview={() => reviewGate(card.id)}
                onOpen={() => openSession(card.id)}
              />
            ))}
          </div>
        </>
      )}

      {/* ── Active now ── */}
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
        Active now
      </h2>
      {active.length === 0 ? (
        <p
          style={{
            fontSize: "calc(15px * var(--fs,1))",
            color: "var(--txt3)",
            margin: "0 0 26px",
          }}
        >
          No sessions actively running right now.
        </p>
      ) : (
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: 8,
            marginBottom: 26,
          }}
        >
          {active.map((card) => (
            <ActiveRow
              key={card.id}
              card={card}
              onClick={() => openSession(card.id)}
            />
          ))}
        </div>
      )}

      {/* ── Idle & finished ── */}
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
        Idle &amp; finished
      </h2>
      {idle.length === 0 ? (
        <p style={{ fontSize: "calc(15px * var(--fs,1))", color: "var(--txt3)", margin: 0 }}>
          No idle or finished sessions.
        </p>
      ) : (
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "repeat(auto-fill, minmax(260px, 1fr))",
            gap: 10,
          }}
        >
          {idle.map((card) => (
            <IdleCard
              key={card.id}
              card={card}
              onClick={() => openSession(card.id)}
            />
          ))}
        </div>
      )}
    </div>
  );
}
