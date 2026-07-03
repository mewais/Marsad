/**
 * TypeScript mirrors of marsad-viewmodel Rust structs.
 * Field names are snake_case to match serde JSON output exactly.
 * Phase serializes as the variant name string.
 */

import type { BoardView } from "./workstreamTypes";

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

export interface QuestionOption {
  label: string;
  description: string;
}

export interface QuestionPrompt {
  question: string;
  header: string;
  multi_select: boolean;
  options: QuestionOption[];
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
  | { Edit: { file: string; diff: DiffLine[] } }
  | {
      Question: {
        request_id: string;
        questions: QuestionPrompt[];
      };
    };

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
// McpServerInfo / SlashCommandInfo (from system/init)
// ---------------------------------------------------------------------------

export interface McpServerInfo {
  name: string;
  status: string;
}

export interface SlashCommandInfo {
  name: string;
  description: string;
  argument_hint: string;
}

// ---------------------------------------------------------------------------
// SessionView — raw from core
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// ModelUsageRow — one row in the per-model usage breakdown
// ---------------------------------------------------------------------------

export interface ModelUsageRow {
  model: string;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
  /**
   * U4 cost truthfulness: `true` when this row's cost was recomputed from
   * the local litellm/hand pricing table (transcript backfill / usage
   * probe) rather than copied verbatim from a live, agent-reported
   * `result.modelUsage` entry. Drives the "estimated"/"~" affordance in the
   * Usage tab's per-model breakdown table.
   */
  estimated: boolean;
}

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
  /** Number of currently running (in-progress) subagents. */
  running_subagents: number;
  cost_usd: number;
  tokens: number;
  context_pct: number;
  gate: Gate | null;
  transcript: TranscriptEntry[];
  changes: FileChange[];
  model_usage: ModelUsageRow[];
  /**
   * U4 cost truthfulness: `true` when `cost_usd` has never been set by a
   * live agent-reported `result` event and is backed only by the
   * litellm/hand pricing table (transcript backfill / usage probe) — i.e.
   * every row in `model_usage` is `estimated`. `false` once any live
   * `result` has landed (or before any cost data exists at all). Drives the
   * Usage tab's "estimated"/"~" affordance on the top Cost tile.
   */
  cost_estimated: boolean;
  /** Tools available in this session (from system/init). Empty before first init. */
  tools: string[];
  /** MCP servers connected in this session (from system/init). */
  mcp_servers: McpServerInfo[];
  /** Slash commands available in this session (from system/init). */
  slash_commands: SlashCommandInfo[];
  /** Permission posture: "ask" | "auto" | "full" (from system/init permissionMode). */
  posture: string;
  /** Count of context compactions this session (system/compact_boundary events). */
  compactions: number;
  /** Metadata of the most recent compaction, if any. */
  last_compaction: CompactMetadata | null;
  /** Most recent rate-limit info, if the session has hit one. */
  rate_limit: RateLimitInfo | null;
}

/** Mirrors marsad_proto::CompactMetadata — one context-compaction event. */
export interface CompactMetadata {
  trigger: string;
  pre_tokens: number;
  post_tokens: number;
  cumulative_dropped_tokens: number;
  duration_ms: number;
}

/** Mirrors marsad_proto::RateLimitInfo — a rate_limit_event's payload. */
export interface RateLimitInfo {
  status: string;
  resets_at: number;
  rate_limit_type: string;
  overage_status: string;
  is_using_overage: boolean;
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
  /**
   * U4 cost truthfulness: `true` when every session contributing to `cost`
   * is backfill-only (`cost_estimated: true`) — i.e. no live agent-reported
   * cost has landed for ANY of them yet. Drives the "~"/"estimated"
   * affordance on the Overview "Cost today" tile.
   */
  cost_estimated: boolean;
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
  /** Completed subagent count (mirrors SessionView.subagents). */
  subagents: number;
  /** Currently-running subagent count (mirrors SessionView.running_subagents). */
  running_subagents: number;
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
  is_question: boolean;
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
// Workstream — mirrors marsad-workstream::Workstream (id/name/goal/profile)
// ---------------------------------------------------------------------------

export interface Workstream {
  id: string;
  name: string;
  goal: string;
  profile?: string | null;
}

// ---------------------------------------------------------------------------
// Profile — mirrors marsad-workstream::Profile
// ---------------------------------------------------------------------------

export interface Profile {
  id: string;
  name: string;
}

// ---------------------------------------------------------------------------
// Connection — mirrors marsad-workstream::Connection
// ---------------------------------------------------------------------------

export interface Connection {
  id: string;
  ws: string;
  from: string;
  to: string;
  kind: "data" | "command" | "both";
  bi: boolean;
  has_gate?: boolean;
}

// ---------------------------------------------------------------------------
// Capabilities — TS mirror of marsad-proto's Capabilities/ModelCatalog/
// DiscoveredSession/SessionScope + marsad-viewmodel's CapabilitiesView
// (U1/U2, docs/specs/dynamic-capabilities.md §1/§3.6). Field names/shapes are
// verbatim mirrors of the Rust serde JSON — no renaming, no label synthesis.
// ---------------------------------------------------------------------------

/** Provenance of a model catalog — drives the UI's "unverified list" treatment. */
export type ModelListSource = "Listed" | "Observed";

/** One selectable model, exactly as the driver reports it. Ids are verbatim —
 *  never normalized (agy model names contain spaces/parens). */
export interface ModelInfo {
  id: string;
  annotation?: string | null;
}

export interface ModelCatalog {
  models: ModelInfo[];
  source: ModelListSource;
  /** UI must additionally offer a free-text model field when true. */
  free_text: boolean;
}

/** One resumable past session discovered on the host. */
export interface DiscoveredSession {
  id: string;
  /** None where the store doesn't record it retrievably (e.g. agy). */
  project?: string | null;
  snippet: string;
  modified_ms: number;
}

/** Serializes as the bare string `"AllKnown"` or `{"Projects": [...]}`. */
export type SessionScope = "AllKnown" | { Projects: string[] };

/** Display-ready mirror of `marsad_viewmodel::CapabilitiesView` — the unit of
 *  caching/wire-transfer/VmUpdate for one (driver, host) pair. */
export interface CapabilitiesView {
  driver: string;
  host_id: string;
  cli_available: boolean;
  models: ModelCatalog;
  session_scope: SessionScope | null;
  sessions: DiscoveredSession[];
  /** Unix epoch ms of the last successful probe; 0 = never (honest unknown). */
  fetched_at_ms: number;
  errors: string[];
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
  | { kind: "workspaces"; workspaces: Workspace[] }
  | { kind: "workstreams"; workstreams: Workstream[] }
  | { kind: "board"; board: BoardView }
  | { kind: "capabilities"; capabilities: CapabilitiesView }
  | { kind: "wasl_gates"; gates: WaslGateView[] }
  | {
      kind: "wasl_gate_decided";
      host_id: string;
      gate_id: string;
      outcome: string;
      decided_by: string;
      reason: string | null;
    }
  | { kind: "wasl_observation"; host_id: string; entry: WaslAuditEntry };

/**
 * X5 — a pending Wasl handoff gate surfaced to the human (mirrors
 * marsad-core WaslGateView). Marsad displays it and RELAYS the human's
 * approve/deny; it never auto-answers.
 */
export interface WaslGateView {
  host_id: string;
  gate_id: string;
  kind: string;
  from: string;
  summary: string;
  preview: string;
  requested_at: string;
  deadline: string;
}

/** One Wasl observation-feed entry (mirrors marsad_wasl::AuditEntry). */
export interface WaslAuditEntry {
  seq: number;
  kind: string;
  actor: string;
  targets: string[];
  summary: string;
}
