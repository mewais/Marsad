/**
 * TypeScript mirrors of marsad-viewmodel Rust structs.
 * Field names are snake_case to match serde JSON output exactly.
 * Phase serializes as the variant name string.
 */

// ---------------------------------------------------------------------------
// Phase (marsad-events)
// ---------------------------------------------------------------------------

export type Phase =
  | "Needs"
  | "Working"
  | "Error"
  | "Done"
  | "Starting"
  | "Stale"
  | "Detached";

// ---------------------------------------------------------------------------
// Gate (marsad-gates)
// ---------------------------------------------------------------------------

export type Sig = "Context" | "Add" | "Del";

export interface DiffLine {
  sig: Sig;
  ln: string;
  text: string;
}

export interface PlanStep {
  title: string;
  detail: string;
}

export type Gate =
  | {
      Permission: {
        request_id: string;
        command: string;
        cwd: string;
        risk: string;
        explanation: string;
      };
    }
  | { Plan: { summary: string; steps: PlanStep[] } }
  | { Edit: { file: string; diff: DiffLine[] } };

// ---------------------------------------------------------------------------
// Host (marsad-hosts)
// ---------------------------------------------------------------------------

export type HostStatus =
  | "Connected"
  | "Connecting"
  | "Reconnecting"
  | "Unreachable";

export interface Host {
  id: string;
  label: string;
  target: string;
  status: HostStatus;
  session_count: number;
  is_local: boolean;
  removable: boolean;
  /** Per-host claude executable (wrapper / absolute path); null → default `claude`. */
  claude_bin?: string | null;
  /** Per-host setup shell command run before claude; null → none. */
  setup?: string | null;
  /** Per-host environment, as `[key, value]` pairs. */
  env?: Array<[string, string]>;
}

// ---------------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------------

export interface Workspace {
  id: string;
  name: string;
  session_ids: string[];
  saved_at: string;
}

// ---------------------------------------------------------------------------
// Filter / Tab
// ---------------------------------------------------------------------------

export type Filter = "All" | "Needs" | "Working" | "Error";
export type Tab = "Activity" | "Changes" | "Config" | "Usage" | "Terminal";

// ---------------------------------------------------------------------------
// SessionView — raw from core
// ---------------------------------------------------------------------------

export interface SessionView {
  id: string;
  name: string;
  host_id: string;
  project: string;
  branch: string;
  model: string;
  state: Phase;
  changed_files: number;
  subagents: number;
  cost_usd: number;
  tokens: number;
  context_pct: number;
  gate: Gate | null;
  transcript: TranscriptEntry[];
  changes: FileChange[];
}

// ---------------------------------------------------------------------------
// TranscriptEntry — one rendered conversation line (matches Rust, role-tagged)
// ---------------------------------------------------------------------------

export type TranscriptEntry =
  | { role: "assistant"; text: string }
  | { role: "user"; text: string }
  | { role: "tool"; name: string; input: string };

// ---------------------------------------------------------------------------
// FileChange — one changed file from the git probe
// ---------------------------------------------------------------------------

export interface FileChange {
  path: string;
  status: string;
  added: number;
  removed: number;
}

// ---------------------------------------------------------------------------
// AppSnapshot
// ---------------------------------------------------------------------------

export interface AppSnapshot {
  sessions: SessionView[];
  hosts: Host[];
  workspaces: Workspace[];
  active_ws: string | null;
  selected_id: string | null;
  filter: Filter;
  search: string;
  tab: Tab;
}

// ---------------------------------------------------------------------------
// Counts (computed from snapshot on TS side for display)
// ---------------------------------------------------------------------------

export interface Counts {
  total: number;
  needs: number;
  errors: number;
  working: number;
  cost: string;
  tokens: string;
  host_count: number;
}

// ---------------------------------------------------------------------------
// SessionCard / SessionRow (display-ready)
// ---------------------------------------------------------------------------

export interface SessionCard {
  id: string;
  name: string;
  state: Phase;
  host_label: string;
  project: string;
  branch: string;
  gate_kind: string | null;
  gate_summary: string | null;
  last_action: string | null;
  context_pct: number;
  tokens_label: string;
  elapsed_label: string;
}

export type SessionRow = SessionCard;

// ---------------------------------------------------------------------------
// OverviewGroups
// ---------------------------------------------------------------------------

export interface OverviewGroups {
  attention: SessionCard[];
  active: SessionCard[];
  idle: SessionCard[];
}

// ---------------------------------------------------------------------------
// SessionDetailVM
// ---------------------------------------------------------------------------

export interface SessionDetailVM {
  name: string;
  host_label: string;
  branch: string;
  model: string;
  ctx_warn: boolean;
  context_pct: number;
  tokens_label: string;
  cost_label: string;
  elapsed_label: string;
}

// ---------------------------------------------------------------------------
// GateVM
// ---------------------------------------------------------------------------

export interface SideCell {
  ln: string;
  sig: string;
  sigColor: string;
  text: string;
  fg: string;
  bg: string;
}

export interface SideRow {
  l: SideCell;
  r: SideCell;
}

export interface GateVM {
  kind: string;
  icon: string;
  lead: string;
  summary: string;
  /** Unique id for THIS gate occurrence — the control-request id for a
   *  permission gate. Used as the decision id so distinct gates in one session
   *  aren't collapsed by idempotency dedup. */
  decision_id: string;
  is_permission: boolean;
  is_plan: boolean;
  is_edit: boolean;
  diff_rows: SideRow[];
}

// ---------------------------------------------------------------------------
// Settings — mirrors PersistedSettings / NotifPref in marsad-store
// ---------------------------------------------------------------------------

export interface NotifPref {
  enabled: boolean;
  priority: "urgent" | "normal" | "low";
  sound: boolean;
  group: boolean;
  dnd: boolean;
}

export interface Settings {
  /** Per-event notification prefs, keyed by NotifEvent name. */
  notif_prefs: Record<string, NotifPref>;
  play_sound: boolean;
  group_alerts: boolean;
  do_not_disturb: boolean;
  background: string;
  restore_on_launch: boolean;
  confirm_before_stop: boolean;
  default_model: string;
  default_posture: "ask" | "auto" | "full";
}

// ---------------------------------------------------------------------------
// SlashEntry
// ---------------------------------------------------------------------------

export interface SlashEntry {
  name: string;
  desc: string;
}

// ---------------------------------------------------------------------------
// SessionSummary — returned by list_sessions command
// ---------------------------------------------------------------------------

export interface SessionSummary {
  id: string;
  snippet: string;
  modified: number; // epoch ms
}

// ---------------------------------------------------------------------------
// PersistedTemplate — named config template (model + posture)
// ---------------------------------------------------------------------------

export interface PersistedTemplate {
  name: string;
  model: string;
  posture: "ask" | "auto" | "full";
}

// ---------------------------------------------------------------------------
// VmUpdate event from Tauri
// ---------------------------------------------------------------------------

export type VmUpdateEvent =
  | { kind: "overview"; groups: OverviewGroups }
  | { kind: "session"; session: SessionView }
  | { kind: "session_removed"; id: string }
  | { kind: "toast"; icon: string; text: string }
  | { kind: "host_health"; host_id: string; status: string }
  | { kind: "hosts"; hosts: Host[] }
  | { kind: "workspaces"; workspaces: Workspace[] };
