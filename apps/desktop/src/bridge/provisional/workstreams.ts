/**
 * PROVISIONAL ADAPTER — the one place the UI assumes workstream shapes the generated wire does not
 * define yet. Replace with generated types when they land.
 *
 * 1. Wasl types (`wasl-proto`, hand-written here because they are not exported to TS): topology,
 *    edge policy, tasks and their states, served by each host's Wasl node as `wasl.*`.
 * 2. The workstream *document*: `store.workstreams.*` stores an opaque JSON `doc` with a schema
 *    number chosen by its writer (marsad-proto `WorkstreamDoc`). Schema 1 below is the UI's own.
 */
import type { JsonValue, Usage, WorkstreamDoc } from "../wire";

// ── wasl-proto ──────────────────────────────────────────────────────────────

/** `"<host>/<session>"`. */
export type AgentId = string;
export type PeerStatus = "starting" | "idle" | "working" | "needs_you" | "stopped" | "failed" | "unreachable" | "unknown";
export type EdgePolicy = "ask" | "always_allow" | "deny";
export interface PeerCard { agent: AgentId; name: string; driver: string; role: string; description: string; status: PeerStatus }
export interface Edge { from: AgentId; to: AgentId; policy: EdgePolicy }
export interface Limits { max_depth: number; default_deadline_secs: number; budget_usd?: number }
export interface Topology { workstream: string; version: number; peers: PeerCard[]; edges: Edge[]; limits: Limits; paused?: boolean }
export type TaskMode = "consult" | "command" | "message";
export interface Task {
  id: string; workstream: string; from: AgentId; to: AgentId; mode: TaskMode; instruction: string;
  attachments?: string[]; deadline_ms: number; created_ms: number; chain: AgentId[]; hop: number; parent?: string;
}
export interface Progress { tool_calls: number; last_tool?: string; last_text?: string }
export interface TaskResult { status: "success" | "error"; summary: string; artifacts?: { name: string; uri: string }[]; usage?: Usage }
export type TaskState =
  | { state: "pending_approval"; via: "gate" | "elicitation" }
  | { state: "queued" }
  | { state: "running"; progress: Progress }
  | { state: "completed"; result: TaskResult }
  | { state: "failed"; error: string }
  | { state: "cancelled"; reason: string }
  | { state: "denied"; reason: string }
  | { state: "expired" };
export interface TaskView { task: Task; state: TaskState; updated_ms: number }
/** Payload of `wasl.tasks` (`TaskEvent`: side + flattened view). */
export type TaskEvent = TaskView & { side: "outbound" | "inbound" };
export interface TaskList { tasks: TaskView[] }

export const waslMethods = { TOPOLOGY_SET: "wasl.topology.set", TOPOLOGY_GET: "wasl.topology.get", TASKS_LIST: "wasl.tasks.list" } as const;
export const waslTopics = { TASKS: "wasl.tasks", EDGES: "wasl.edges" } as const;

export const isTerminal = (s: TaskState) => ["completed", "failed", "cancelled", "denied", "expired"].includes(s.state);

// ── workstream document, schema 1 (UI-defined) ──────────────────────────────

export const WS_SCHEMA = 1;

export interface WsNode {
  /** Node id inside the workstream (also `WorkstreamNodeRef.node`). */
  id: string;
  name: string;
  host: string;
  driver: string;
  cwd: string;
  model?: string | null;
  mode?: string | null;
  first_prompt?: string | null;
  /** Native session to continue (pulled-in existing session). */
  resume?: string | null;
  /** The Marsad session once launched (or the existing one pulled in). */
  session?: string | null;
  /** Manual position on the canvas (overrides the automatic layout). */
  pos?: { x: number; y: number } | null;
}
export interface WsEdge { id: string; from: string; to: string; policy: EdgePolicy }
export interface WsDocV1 {
  name: string;
  description: string;
  status: "draft" | "launched";
  nodes: WsNode[];
  edges: WsEdge[];
  limits?: Partial<Limits>;
}
export interface Workstream extends WsDocV1 {
  id: string;
  revision: number;
  updatedMs: number;
}

/** Read a stored document; unknown schemas are skipped (another client wrote them). */
export function readWorkstream(d: WorkstreamDoc): Workstream | null {
  if (d.schema !== WS_SCHEMA || !d.doc || typeof d.doc !== "object") return null;
  const doc = d.doc as unknown as Partial<WsDocV1>;
  return {
    id: d.id,
    revision: d.revision,
    updatedMs: d.updated_at_ms,
    name: doc.name ?? d.id,
    description: doc.description ?? "",
    status: doc.status ?? "draft",
    nodes: doc.nodes ?? [],
    edges: doc.edges ?? [],
    limits: doc.limits,
  };
}

export function writeWorkstream(w: Workstream): { id: string; schema: number; doc: JsonValue; expected_revision: number | null } {
  const doc: WsDocV1 = { name: w.name, description: w.description, status: w.status, nodes: w.nodes, edges: w.edges, limits: w.limits };
  return { id: w.id, schema: WS_SCHEMA, doc: doc as unknown as JsonValue, expected_revision: w.revision || null };
}

export const agentIdOf = (n: Pick<WsNode, "host" | "session">): AgentId | null => (n.session ? `${n.host}/${n.session}` : null);

/** The Wasl topology a launched workstream pushes to its nodes. */
export function toTopology(w: Workstream, version: number): Topology {
  const byId = new Map(w.nodes.map((n) => [n.id, n]));
  const aid = (id: string) => agentIdOf(byId.get(id) ?? { host: "", session: null });
  return {
    workstream: w.id,
    version,
    peers: w.nodes.filter((n) => n.session).map((n) => ({ agent: agentIdOf(n)!, name: n.name, driver: n.driver, role: "", description: "", status: "unknown" })),
    edges: w.edges.filter((e) => aid(e.from) && aid(e.to)).map((e) => ({ from: aid(e.from)!, to: aid(e.to)!, policy: e.policy })),
    limits: { max_depth: 4, default_deadline_secs: 1800, ...w.limits },
  };
}
