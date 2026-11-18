import type { PendingGate } from "../../bridge/wire";
import type { SessionView } from "../../state";
import { HandoffGate } from "./HandoffGate";
import { PermissionGate } from "./PermissionGate";
import { QuestionGate } from "./QuestionGate";
import "./gates.css";

/** Where the gate is drawn: Inbox row or the session view. */
export type GateDensity = "row" | "full";

export interface GateProps {
  view: SessionView;
  pg: PendingGate;
  density: GateDensity;
  /** This gate owns the answering shortcuts (digits, A, D, Enter). */
  active?: boolean;
  /** Rows: show the expanded form instead of the one-line summary. */
  expanded?: boolean;
  /** Rows: ask the list to select this row (expand). */
  onActivate?: () => void;
}

/** The one gate component family used by Inbox rows and the session view. */
export function Gate({ view, pg, density, active = false, expanded = false, onActivate }: GateProps) {
  const common = { view, density, active, expanded, onActivate };
  switch (pg.gate.gate_type) {
    case "question":
      return <QuestionGate {...common} pg={pg as Parameters<typeof QuestionGate>[0]["pg"]} />;
    case "handoff":
      return <HandoffGate {...common} pg={pg as Parameters<typeof HandoffGate>[0]["pg"]} />;
    default:
      return <PermissionGate {...common} pg={pg as Parameters<typeof PermissionGate>[0]["pg"]} />;
  }
}
