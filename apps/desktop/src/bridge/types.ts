/**
 * PROVISIONAL wire types. TO BE REPLACED BY GENERATED TYPES (`src/bridge/generated/`).
 *
 * Transcribed by hand from the Rust crates (field names and serde renames exactly):
 *   - wasit-runtime/src/wire.rs   (SessionSnapshot, Phase, PendingGate, TurnInfo, SessionEvent, ...)
 *   - wasit-core/src/event.rs     (AgentEvent, GateRequest, GateDecision, QuestionAnswer, Usage, ...)
 *   - wasit-core/src/protocol.rs  (Command)
 *   - wasit-core/src/descriptor.rs (DriverId, DriverCapabilities)
 *
 * Anything marked PROVISIONAL below is NOT in the Rust types today: it is what the UI needs and
 * must be settled with the backend (listed in the hand-off report).
 */

// ── ids ─────────────────────────────────────────────────────────────────────

/** 12 chars of lowercase base32 (`a-z2-7`). Serde-transparent string. */
export type SessionId = string;
/** Serde-transparent string: "claude", "codex", "opencode", "kilo", ... */
export type DriverId = string;
/** Frontend-only: which machine a session lives on. */
export type HostId = string;
/** Frontend-only: `${hostId}/${sessionId}`. */
export type SessionKey = string;

export const sessionKey = (host: HostId, id: SessionId): SessionKey => `${host}/${id}`;
export function splitKey(key: SessionKey): { host: HostId; id: SessionId } {
  const i = key.indexOf("/");
  return { host: key.slice(0, i), id: key.slice(i + 1) };
}

// ── wasit-core: usage, catalogs ─────────────────────────────────────────────

export interface Usage {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_creation_tokens: number;
  /** 0 when the CLI does not report cost. */
  cost_usd: number;
}

export interface ModelUsage {
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
  estimated: boolean;
}

export interface SessionMode {
  id: string;
  label: string;
  description: string;
}

export interface ModelInfo {
  id: string;
  display_name: string;
  description: string;
  is_default: boolean;
}

export interface McpServerInfo {
  name: string;
  status: string;
}

/** `name` has no leading slash. */
export interface SlashCommandInfo {
  name: string;
  description: string;
  argument_hint: string;
}

export interface PlanEntry {
  content: string;
  priority: string;
  /** "pending" | "in_progress" | "completed" when the CLI reports it. */
  status?: string;
}

export interface CompactionInfo {
  trigger: string;
  pre_tokens: number;
  post_tokens: number;
  cumulative_dropped_tokens: number;
  duration_ms: number;
}

export interface RateLimitInfo {
  status: string;
  resets_at: number;
  rate_limit_type: string;
  overage_status: string;
  is_using_overage: boolean;
}

export type EndReason = "cancelled" | "max_turns" | "execution_error" | "compaction_needed";

export type BusyPrompt = "queued_by_cli" | "steered" | "queued_by_protocol";

export interface DriverCapabilities {
  interrupt: boolean;
  set_model: boolean;
  set_mode: boolean;
  slash_commands: boolean;
  permission_gates: boolean;
  questions: boolean;
  plan_gate: boolean;
  usage: boolean;
  cost: boolean;
  resume: boolean;
  continue_latest: boolean;
  streaming_output: boolean;
  prompt_while_busy: BusyPrompt;
}

// ── wasit-core: gates ───────────────────────────────────────────────────────

export interface ElicitationChoice {
  key: string;
  value: string;
  label: string;
}

export interface QuestionOption {
  label: string;
  description: string;
}

export interface QuestionPrompt {
  question: string;
  header: string;
  multi_select: boolean;
  options?: QuestionOption[];
}

/**
 * PROVISIONAL: the agent's own wording for the buttons of a permission gate. Not in wire.rs today;
 * the UI needs it to show "Yes / Yes, and don't ask again for ... / No, and tell Claude ..." (or
 * Codex's "Approve / Approve for this session / Deny") verbatim. See gates/choices.ts for how the
 * UI falls back when absent.
 */
export interface PermissionChoice {
  id: string;
  label: string;
  /** Which button style / decision this maps to. */
  kind: "allow" | "always" | "deny";
  /** `deny` choices that let the human add a reason ("tell the agent what to do differently"). */
  asks_reason?: boolean;
}

/** `gate_type`-tagged. */
export type GateRequest =
  | {
      gate_type: "permission";
      request_id: string;
      tool_name: string;
      command: string;
      cwd: string;
      explanation?: string;
      is_elicitation?: boolean;
      elicitation_choices?: ElicitationChoice[];
      permission_suggestions?: unknown[];
      raw_input?: unknown;
      /** PROVISIONAL, see PermissionChoice. */
      choices?: PermissionChoice[];
    }
  | { gate_type: "plan"; request_id: string; summary: string; choices?: PermissionChoice[] }
  | { gate_type: "question"; request_id: string; questions: QuestionPrompt[] }
  /** PROVISIONAL: a workstream handoff (agent to agent) waiting for the human. Not a Wasit gate. */
  | {
      gate_type: "handoff";
      request_id: string;
      from_name: string;
      to_name: string;
      task: string;
      payload?: string;
    };

export type GateDecision =
  | {
      action: "approve";
      updated_input?: unknown;
      permission_updates?: unknown;
      elicitation_content?: unknown;
    }
  | { action: "deny"; reason?: string };

export interface QuestionAnswer {
  /** question text -> selected label(s), comma-joined for multi-select. Empty = skip. */
  answers: Record<string, string>;
}

// ── wasit-core: AgentEvent (`type`-tagged) ──────────────────────────────────

export type AgentEvent =
  | {
      type: "session_init";
      native_session_id?: string;
      model?: string;
      cwd?: string;
      tools?: string[];
      mcp_servers?: McpServerInfo[];
      slash_commands?: SlashCommandInfo[];
      available_modes?: SessionMode[];
      current_mode?: string;
      cli_version?: string;
    }
  | { type: "turn_started"; native_turn_id?: string }
  | {
      type: "turn_completed";
      end_reason?: EndReason;
      was_interrupted: boolean;
      num_turns: number;
      usage: Usage;
      per_model?: Record<string, ModelUsage>;
      reply?: string;
    }
  | { type: "assistant_delta"; text: string; message_id?: string }
  | { type: "thinking_delta"; text: string; message_id?: string }
  | {
      type: "tool_call_started";
      name: string;
      call_id: string;
      input: unknown;
      is_subagent: boolean;
      subagent_model?: string;
    }
  | {
      type: "tool_call_updated";
      call_id: string;
      name: string;
      content?: unknown[];
      status?: string;
    }
  | { type: "tool_call_finished"; call_id: string; name: string; ok: boolean; result?: unknown }
  | { type: "gate_requested"; gate: GateRequest }
  | { type: "gate_resolved"; request_id: string }
  | { type: "usage_update"; usage: Usage; context_pct?: number }
  | { type: "plan_updated"; entries: PlanEntry[] }
  | { type: "modes_updated"; available_modes?: SessionMode[]; current_mode?: string }
  | { type: "models_updated"; models: ModelInfo[]; current?: string }
  | { type: "model_changed"; model: string }
  | { type: "slash_commands_updated"; commands: SlashCommandInfo[] }
  | { type: "compaction"; info: CompactionInfo }
  | { type: "rate_limit_notice"; info: RateLimitInfo }
  | { type: "command_rejected"; command: string; message: string }
  | { type: "notice"; message: string }
  | { type: "process_exited"; code?: number }
  | { type: "unknown"; raw: unknown };

// ── wasit-core: Command (`command`-tagged) ──────────────────────────────────

export type Command =
  | { command: "prompt"; text: string }
  | { command: "answer_gate"; request_id: string; decision: GateDecision }
  | { command: "answer_question"; request_id: string; answer: QuestionAnswer }
  | { command: "interrupt" }
  | { command: "set_model"; model: string }
  | { command: "set_mode"; mode: string }
  | { command: "run_slash"; name: string; args: string }
  | { command: "shutdown" };

// ── wasit-runtime: phase, snapshot, events ──────────────────────────────────

/** `state`-tagged. */
export type Phase =
  | { state: "starting" }
  | { state: "idle" }
  | { state: "working" }
  | { state: "needs_you" }
  | { state: "exited"; code?: number }
  | { state: "failed"; reason: string; stderr_tail?: string[] }
  | { state: "lost"; reason: string };

export type PhaseKind = Phase["state"];

export interface PendingGate {
  gate: GateRequest;
  raised_at_ms: number;
  cursor: number;
}

export interface TurnInfo {
  started_at_ms: number;
  native_turn_id?: string;
  prompt?: string;
  tool_calls: number;
  last_tool?: string;
}

export interface TurnOutcome {
  ended_at_ms: number;
  end_reason?: EndReason;
  was_interrupted: boolean;
  num_turns: number;
  reply?: string;
}

export interface SubagentInfo {
  call_id: string;
  name: string;
  model?: string;
  description?: string;
  started_at_ms: number;
  running: boolean;
  ok?: boolean;
}

export interface ExitSummary {
  code?: number;
  signal?: number;
  at_ms: number;
  requested: boolean;
}

export interface SessionSnapshot {
  id: SessionId;
  driver: DriverId;
  cwd?: string;
  labels?: Record<string, string>;
  created_at_ms: number;
  capabilities: DriverCapabilities;
  phase: Phase;
  generation: number;
  pid?: number;
  native_session_id?: string;
  /** The first prompt, truncated. */
  title?: string;
  cli_version?: string;
  model?: string;
  mode?: string;
  available_models: ModelInfo[];
  available_modes: SessionMode[];
  slash_commands: SlashCommandInfo[];
  pending_gates: PendingGate[];
  turn?: TurnInfo;
  last_turn?: TurnOutcome;
  turns_completed: number;
  usage: Usage;
  live_usage?: Usage;
  context_pct?: number;
  subagents: SubagentInfo[];
  plan: PlanEntry[];
  last_activity_ms: number;
  exit?: ExitSummary;
  stderr_tail: string[];
  cursor: number;
  loaded: boolean;
}

export type CommandStatus =
  | { status: "sent" }
  | { status: "queued" }
  | { status: "unsupported"; operation: string }
  | { status: "rejected"; reason: string }
  | { status: "not_running"; reason: string };

/** Flattened: `{cursor, t, kind, ...payload}`. */
export type SessionEventKind =
  | { kind: "spawned"; generation: number; pid?: number; argv?: string[] }
  | { kind: "agent"; event: AgentEvent }
  | { kind: "input"; command: Command; status: CommandStatus }
  | { kind: "stderr"; text: string }
  | { kind: "phase"; phase: Phase }
  | { kind: "exited"; code?: number; signal?: number }
  | { kind: "lost"; reason: string }
  | { kind: "notice"; message: string };

export type SessionEvent = { cursor: number; t: number } & SessionEventKind;

// ── requests / replies ──────────────────────────────────────────────────────

export interface SessionRef {
  id: SessionId;
}
export interface SessionList {
  sessions: SessionSnapshot[];
}
export interface HistoryRequest {
  id: SessionId;
  before?: number;
  limit: number;
}
export interface HistoryPage {
  /** Oldest first. */
  events: SessionEvent[];
  has_more: boolean;
}
export interface CommandRequest {
  id: SessionId;
  command: Command;
}
export type CommandOutcome = "sent" | "queued";
export interface CommandReply {
  outcome: CommandOutcome;
}
export type ResumeRequest = { kind: "new" } | { kind: "continue"; native_session_id: string };
export interface CreateSession {
  driver: DriverId;
  cwd?: string;
  model?: string;
  mode?: string;
  first_prompt?: string;
  resume?: ResumeRequest;
  config?: Record<string, unknown>;
  labels?: Record<string, string>;
}
export interface CreateSessionReply {
  id: SessionId;
  unsupported: unknown[];
}
export type Ack = Record<string, never>;

/** `kind`-tagged. */
export type HostEvent =
  | { kind: "reset"; sessions: SessionSnapshot[] }
  | { kind: "upsert"; session: SessionSnapshot }
  | { kind: "removed"; id: SessionId };

export interface DriverSummary {
  id: DriverId;
  display_name: string;
  capabilities: DriverCapabilities;
}

export interface HostInfo {
  host_id: string;
  build: string;
  pid: number;
  started_at_ms: number;
  data_dir: string;
  os: string;
  arch: string;
  drivers: DriverSummary[];
}

export interface ResumableSession {
  id: string;
  project?: string;
  title: string;
  modified_ms: number;
}

export interface DriverInventory {
  driver: DriverId;
  display_name: string;
  capabilities: DriverCapabilities;
  installed: boolean;
  binary_path?: string;
  error?: string;
  version?: string;
  models: ModelInfo[];
  modes: SessionMode[];
  sessions: ResumableSession[];
  project?: string;
  refreshing: boolean;
  probes: { kind: string; ok: boolean; error?: string; duration_ms: number }[];
  refreshed_at_ms: number;
}
export interface InventoryReply {
  drivers: DriverInventory[];
}

/** Error shape of `request` rejections. `code` is one of wire.rs `codes` / wasit_control codes. */
export interface RpcError {
  code: string;
  message: string;
  data?: unknown;
}

export const methods = {
  HOST_INFO: "host.info",
  DRIVERS_INVENTORY: "drivers.inventory",
  SESSIONS_LIST: "sessions.list",
  SESSIONS_CREATE: "sessions.create",
  SESSIONS_CONTINUE: "sessions.continue",
  SESSIONS_STOP: "sessions.stop",
  SESSION_SNAPSHOT: "session.snapshot",
  SESSION_HISTORY: "session.history",
  SESSION_COMMAND: "session.command",
  /** PROVISIONAL: workstream handoff decision. */
  HANDOFF_DECIDE: "handoff.decide",
} as const;

export const topics = {
  HOST_SESSIONS: "host.sessions",
  sessionEvents: (id: SessionId) => `session.events:${id}`,
} as const;

// ── frontend-only: connection state (not wire) ──────────────────────────────

export type LinkState = "connected" | "connecting" | "reconnecting" | "offline";

export interface HostLink {
  id: HostId;
  name: string;
  /** "local" for the machine the app runs on, "ssh" for remote. */
  where: "local" | "ssh";
  os?: string;
  link: LinkState;
  /** ms until the next reconnect attempt, while `reconnecting`. */
  retry_in_ms?: number;
  error?: string;
}
