/**
 * TypeScript mirrors of marsad-viewmodel Rust structs.
 * Field names are snake_case to match serde JSON output exactly.
 * Phase serializes as the variant name string.
 */

import type { BoardView, Driver } from "./workstreamTypes";

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

/** Why a turn ended, when worth surfacing (mirrors marsad_events::EndReason).
 *  `Cancelled` pairs with `state:"Done"` (calm); the others with `state:"Error"`. */
export type EndReason = "Cancelled" | "MaxTurns" | "ExecutionError";

/** Human-facing text for an EndReason (mirrors `EndReason::text` in Rust). */
export function endReasonText(r: EndReason): string {
  switch (r) {
    case "Cancelled":
      return "Cancelled by the user";
    case "MaxTurns":
      return "Reached the max turn limit";
    case "ExecutionError":
      return "Ended with an execution error";
  }
}

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

/** One scope-of-allowance choice parsed from an MCP elicitation's schema enum. */
export interface ElicitationChoice {
  /** Schema property the value belongs under (e.g. "grant"). */
  key: string;
  /** Enum value sent back in the response content. */
  value: string;
  /** Human-facing label (enumNames when present, else the value). */
  label: string;
}

/** claude's own `permission_suggestion` for a can_use_tool gate — a real
 *  "always allow …" option. Carried VERBATIM (we render a label from claude's
 *  own rule text and echo the whole object back unchanged). Fields mirror
 *  claude's shape; kept loose so we never reshape what claude sent. */
export interface PermissionSuggestion {
  /** e.g. "addRules". */
  type?: string;
  /** e.g. "allow" | "deny". */
  behavior?: string;
  /** e.g. "localSettings" | "session" | … */
  destination?: string;
  /** claude's rule rows, e.g. [{ toolName: "Bash", ruleContent: "git push *" }]. */
  rules?: { toolName?: string; ruleContent?: string }[];
  // Any other keys claude includes are preserved on the object at runtime.
  [k: string]: unknown;
}

export type Gate =
  | {
      Permission: {
        request_id: string;
        command: string;
        cwd: string;
        risk: string;
        explanation: string;
        is_elicitation: boolean;
        /** Scope-of-allowance choices parsed from the elicitation's schema enum
         *  (e.g. once/session/deny). Empty ⇒ render the binary Approve/Reject. */
        elicitation_choices?: ElicitationChoice[];
        /** claude's own `permission_suggestions` for a can_use_tool gate — the real
         *  "always allow …" options, VERBATIM. Each is claude's raw suggestion
         *  object ({type,rules,behavior,destination}); the UI renders one button per
         *  entry and echoes the chosen one back unchanged. Empty ⇒ none offered. */
        permission_suggestions?: PermissionSuggestion[];
      };
    }
  | { Plan: { request_id: string; summary: string; steps: PlanStep[] } }
  | { Edit: { request_id: string; file: string; diff: DiffLine[] } }
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
  /**
   * Per-driver command overrides as `[driver, command]` pairs.
   * E.g. `[["codex", "codex --sandbox"], ["gemini", "/opt/gemini/bin/gemini"]]`.
   * When launching a session, the matching entry is used first; falls back to
   * `claude_bin` for the "claude" driver, or the bare driver name otherwise.
   */
  agent_commands?: Array<[string, string]>;
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

/** One subagent entry from the session's Task/Agent tool-use stream. */
export interface SubagentView {
  name: string;
  model?: string | null;
  /** "Running" or "Done" */
  status: string;
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
  /** Per-subagent detail list (running first, then done). Empty before first Task tool-use. */
  subagent_list: SubagentView[];
  /** True while an async `!` shell command is running (show a working indicator). */
  shell_running?: boolean;
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
  /** The agent driver running THIS session ("claude"/"codex"/…). The header
   *  labels the session by this, not the host's first capability driver. */
  driver?: string;
  /** Count of context compactions this session (system/compact_boundary events). */
  compactions: number;
  /** Metadata of the most recent compaction, if any. */
  last_compaction: CompactMetadata | null;
  /** Most recent rate-limit info, if the session has hit one. */
  rate_limit: RateLimitInfo | null;
  /**
   * Why the last turn ended, when noteworthy: a user Cancel (with `state:"Done"`)
   * or a real failure (`state:"Error"`). Drives the card's reason text so an
   * Error card is never blank and a cancel reads "Cancelled by the user".
   * `null`/absent for a normal turn or while working.
   */
  end_reason?: EndReason | null;
  /**
   * Lifecycle liveness (two-axis model): `true` = Resumed (the process is alive
   * on its host — survives UI close / connection drop / app restart), `false` =
   * Paused (not running, for ANY reason: the user paused it, or the host killed
   * it — reboot / OOM / scheduler / crash). Authoritative from the agent, NOT
   * inferred from `state`: a finished-but-alive session is `state:"Done",
   * live:true`. Drives the Pause↔Resume affordance, the Paused grouping, and
   * spinner suppression. Optional for back-compat with older snapshots; treat
   * a missing value as `true` (Resumed).
   */
  live?: boolean;
  /**
   * Per-session CONNECTION health, independent of `live`: "Connected",
   * "Reconnecting" (stream dropped, auto-reattaching), or "Unstable" (flapping).
   * Driven by the agent's real stream events, not the periodic host poll. The UI
   * shows this and HARD-BLOCKS sending when it's not "Connected" (or when paused).
   * Optional for back-compat; treat a missing value as "Connected".
   */
  conn_health?: "Connected" | "Reconnecting" | "Unstable" | "Disconnected";
  /**
   * ACP plan entries from `session/update{plan}` — the live plan/todo list.
   * Each entry has a `content` string and a `priority` ("critical"|"high"|
   * "medium"|"low"). Optional for back-compat; absent when no plan has been
   * received yet (non-opencode sessions, or opencode sessions in build mode).
   */
  plan_entries?: { content: string; priority: string }[];
  /**
   * Current mode id from ACP `current_mode_update` (e.g. "plan"|"build").
   * Optional for back-compat; absent before the first mode update.
   */
  current_mode?: string;
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

// `at` is epoch milliseconds when known (live entries stamped on arrival;
// resumed entries carry the JSONL time). Absent for entries with no known time.
export type TranscriptEntry =
  | { role: "assistant"; text: string; at?: number | null }
  | { role: "user"; text: string; at?: number | null }
  | { role: "tool"; name: string; input: string; at?: number | null }
  | { role: "compaction"; trigger: string; dropped_tokens: number; at?: number | null };

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
// CostRow / WorkstreamCostBreakdown — mirrors marsad-viewmodel's
// `CostRow`/`WorkstreamCostBreakdown` (SLICE G, crates/marsad-viewmodel/src/lib.rs).
// Powers the Usage tab's "Cost breakdown" — By host / By node.
// ---------------------------------------------------------------------------

/** One row of the workstream cost breakdown ("By host" / "By node"): a
 *  group's total cost, the distinct drivers contributing to it, its share of
 *  the workstream total, and whether it is backed only by estimated
 *  (litellm/hand-table backfill) cost rather than any live, agent-reported
 *  cost. */
export interface CostRow {
  /** Group identity: a host id (`by_host`) or a node/session id (`by_node`). */
  key: string;
  /** Display label: the host id (`by_host`) or the node's display name (`by_node`). */
  label: string;
  /** Distinct drivers contributing to this group, first-appearance order
   *  (a `by_node` row is always a single-element array). */
  driver_dots: Driver[];
  /** Summed `cost_usd` of every contributing session. */
  cost_usd: number;
  /** This group's share of the workstream total, `0..=100`. */
  pct: number;
  /** `true` only when this group has at least one contributing session AND
   *  every one of them is estimated-only (U4 cost truthfulness). */
  estimated: boolean;
}

/** The full cost breakdown for one workstream's sessions, grouped by host and
 *  by node/session (Usage tab's "Cost breakdown" box). */
export interface WorkstreamCostBreakdown {
  /** Sum of `cost_usd` across every session in the workstream. */
  total_cost_usd: number;
  /** `true` only when every session in the workstream is estimated-only. */
  total_estimated: boolean;
  /** Grouped by host id, sorted by cost descending. */
  by_host: CostRow[];
  /** One row per node/session, sorted by cost descending. */
  by_node: CostRow[];
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
  /** True when this permission gate came from an MCP elicitation — the UI hides
   *  "Approve all this session" for these (the button flips claude's posture,
   *  which does not stop the MCP server from re-asking). */
  is_elicitation: boolean;
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
  /**
   * Default agent driver for new sessions ("claude" | "codex" | "gemini").
   * Absent in older persisted settings → defaults to "claude".
   */
  default_driver?: string;
  /**
   * Per-driver default model slugs, as `[driver, model_slug]` pairs.
   * E.g. `[["codex", "codex-mini-latest"]]`.
   * Falls back to `default_model` for the "claude" driver if absent.
   */
  default_models?: Array<[string, string]>;
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
  /** Whether Marsad has a launch driver for this driver (claude/codex → true,
   *  gemini → false) — carried verbatim from `marsad_proto::Capabilities`. A
   *  driver that isn't launchable is never offered even if its CLI is present. */
  launchable: boolean;
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

// ---------------------------------------------------------------------------
// Pty event from Tauri (WAVE 3b escape-hatch terminal) — a SEPARATE event
// channel ("pty", not "vm-update"): raw terminal bytes have no business
// meaning for the board/session/workstream VM the `VmUpdateEvent` union
// above models, so they are not folded into it (see the Rust
// `events::emit_pty_update` doc comment for the full rationale).
// ---------------------------------------------------------------------------

export type PtyEvent =
  /** `data` is base64 of raw pty output bytes (arbitrary — control
   *  sequences, mid-multibyte-UTF-8 reads) — decode and feed straight to
   *  xterm.js's `Terminal.write` (which accepts a `Uint8Array`). */
  | { kind: "pty_output"; session_id: string; terminal_id: string; data: string }
  /** The pty's child process exited; `code` is `-1` if it could not even be
   *  spawned. */
  | { kind: "pty_exit"; session_id: string; terminal_id: string; code: number };

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
