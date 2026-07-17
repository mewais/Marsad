/**
 * GateOptions — the ONE way to render a gate's answer choices.
 *
 * A gate offers the human a set of options to pick from. Those options used to be
 * rendered three different ways inside GateOverlay: claude/ACP permission
 * suggestions as a horizontal row of buttons in the footer, MCP elicitation
 * choices as stacked rows in the body, and question options as stacked rows in
 * the body — three copies of the same button, two layouts. This is the single
 * stacked-row renderer they all use now; the caller just maps its data onto
 * `GateOption[]`. The footer keeps only the terminal actions (approve/reject/…).
 */
import type { CSSProperties } from "react";

/** One selectable answer, normalised from whatever the driver sent. */
export interface GateOption {
  /** React key (also the fallback testid). */
  id: string;
  /** data-testid for the button — preserved verbatim per source. */
  testid?: string;
  /** Primary line (bold). */
  label: string;
  /** Optional secondary line (e.g. a question option's description). */
  description?: string;
  /** Optional native hover title (e.g. an ACP option's destination). */
  title?: string;
  /** Highlight as chosen (radio / multi-select question state). */
  selected?: boolean;
  onSelect: () => void;
}

const rowStyle = (selected: boolean, disabled: boolean): CSSProperties => ({
  display: "flex",
  flexDirection: "column",
  alignItems: "flex-start",
  gap: 2,
  padding: "11px 14px",
  borderRadius: 11,
  border: selected ? "1.5px solid var(--accent2)" : "1px solid var(--border2)",
  background: selected ? "var(--accent-bg)" : "var(--panel2)",
  color: "var(--txt)",
  cursor: disabled ? "not-allowed" : "pointer",
  opacity: disabled ? 0.6 : 1,
  textAlign: "left",
  fontFamily: "inherit",
  width: "100%",
  boxSizing: "border-box",
});

export function GateOptions({
  options,
  disabled = false,
  testid,
}: {
  options: GateOption[];
  disabled?: boolean;
  testid?: string;
}) {
  if (options.length === 0) return null;
  return (
    <div
      data-testid={testid}
      style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 4 }}
    >
      {options.map((o) => (
        <button
          key={o.id}
          data-testid={o.testid}
          onClick={o.onSelect}
          disabled={disabled}
          title={o.title}
          style={rowStyle(!!o.selected, disabled)}
        >
          <span
            style={{
              fontWeight: 700,
              fontSize: "calc(15px * var(--fs,1))",
              color: o.selected ? "var(--accent2)" : "var(--txt)",
            }}
          >
            {o.label}
          </span>
          {o.description && (
            <span
              style={{
                fontSize: "calc(13.5px * var(--fs,1))",
                color: "var(--txt3)",
                lineHeight: 1.4,
              }}
            >
              {o.description}
            </span>
          )}
        </button>
      ))}
    </div>
  );
}
