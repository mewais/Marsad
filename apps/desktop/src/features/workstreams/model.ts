import type { EdgePolicy, EdgeView, HostId, NodeView, SessionSnapshot, TaskView, WorkstreamNode, WorkstreamView } from "../../bridge/wire";
import { phaseToStatus, type StatusKey } from "../../ui/status";
import type { IconName } from "../../ui";

/** Pure presentation of workstreams (list rows, nodes, edges, design checklist). */

export const POLICY: Record<EdgePolicy, { word: string; cls: "ask" | "allow" | "block"; icon: IconName; color: string; desc: string }> = {
  ask: { word: "Ask each time", cls: "ask", icon: "shield", color: "var(--needs)", desc: "Every handoff stops for your approval." },
  always_allow: { word: "Always allow", cls: "allow", icon: "check", color: "var(--done)", desc: "Handoffs go straight through. You still see them here." },
  deny: { word: "Blocked", cls: "block", icon: "ban", color: "var(--failed)", desc: "The agent is told this delegation is not allowed." },
};

export const nodeName = (v: WorkstreamView, id: string) => v.workstream.nodes.find((n) => n.id === id)?.name ?? id;

export type NodeStatus = StatusKey | "planned" | "launching";

export function nodeStatus(nv: NodeView | undefined, s: SessionSnapshot | null | undefined): { status: NodeStatus; word: string } {
  if (nv?.launching) return { status: "launching", word: "Launching" };
  if (!s) return { status: "planned", word: "Not launched" };
  const st = phaseToStatus(s.phase.state, false);
  if (st === "needs" && s.pending_gates.some((g) => g.gate.gate_type === "handoff")) return { status: st, word: "Handoff waiting" };
  return { status: st, word: st === "needs" ? "Needs you" : st === "failed" ? "Failed" : st === "working" ? "Working" : s.phase.state === "exited" ? "Ended" : "Idle" };
}

export interface WsSummary {
  hosts: HostId[];
  sessions: number;
  planned: number;
  parts: { cls: string; text: string; icon: IconName }[];
  cost: number;
  pending: number;
}

export function summary(v: WorkstreamView): WsSummary {
  const hosts = [...new Set(v.nodes.map((n) => n.host))];
  const live = v.nodes.filter((n) => n.session);
  const c: Partial<Record<NodeStatus, number>> = {};
  for (const n of live) {
    const st = nodeStatus(n, n.session).status;
    c[st] = (c[st] ?? 0) + 1;
  }
  const pending = v.edges.reduce((a, e) => a + e.pending_gates.length, 0);
  const parts: WsSummary["parts"] = [];
  const st = v.workstream.state;
  if (st === "draft") parts.push({ cls: "accent", text: "Draft, not launched", icon: "edit" });
  else if (st === "archived") parts.push({ cls: "idle", text: "Archived", icon: "idle" });
  else {
    if (st === "paused") parts.push({ cls: "needs", text: "Paused", icon: "pause" });
    if (c.needs) parts.push({ cls: "needs", text: `${c.needs} need${c.needs === 1 ? "s" : ""} you`, icon: "needs" });
    if (c.failed) parts.push({ cls: "failed", text: `${c.failed} failed`, icon: "failed" });
    if (c.working) parts.push({ cls: "working", text: `${c.working} working`, icon: "working" });
    if (!parts.length) parts.push({ cls: "idle", text: "All idle", icon: "idle" });
  }
  return { hosts, sessions: live.length, planned: v.nodes.length - live.length, parts, cost: v.cost.sessions_usd, pending };
}

/** What moves over an edge, in words. */
export function edgeLabel(e: EdgeView): { text: string; tasks: number; pending: boolean } {
  const pending = e.pending_gates.length > 0;
  const tasks = e.in_flight.filter((t) => t.state.state !== "pending_approval").length;
  return { text: pending ? "Handoff waiting" : POLICY[e.policy].word, tasks, pending };
}

export function taskWords(t: TaskView): string {
  const s = t.state;
  switch (s.state) {
    case "pending_approval":
      return "Waiting for your approval";
    case "queued":
      return "Queued";
    case "running":
      return `${s.progress.tool_calls} tool call${s.progress.tool_calls === 1 ? "" : "s"}${s.progress.last_tool ? ` · ${s.progress.last_tool}` : ""}${s.progress.last_text ? ` · ${s.progress.last_text}` : ""}`;
    case "completed":
      return `Finished${s.result.usage ? ` · $${s.result.usage.cost_usd.toFixed(2)}` : ""}`;
    case "failed":
      return `Failed: ${s.error}`;
    case "cancelled":
      return `Cancelled: ${s.reason}`;
    case "denied":
      return `Denied: “${s.reason}”`;
    case "expired":
      return "Expired before it was picked up";
  }
}

export interface CheckItem {
  ok: "done" | "warn" | "info" | "bad";
  text: string;
  detail: string;
}

/** The "Before you launch" checklist of a draft. */
export function checklist(v: WorkstreamView, hostUp: (h: HostId) => boolean, hostName: (h: HostId) => string): CheckItem[] {
  const w = v.workstream;
  const out: CheckItem[] = [];
  const hosts = [...new Set(v.nodes.map((n) => n.host))];
  out.push(
    w.nodes.length
      ? { ok: "done", text: `${w.nodes.length} agent${w.nodes.length === 1 ? "" : "s"} on ${hosts.length} host${hosts.length === 1 ? "" : "s"}`, detail: w.nodes.map((n) => `${n.name}${n.session ? " (keeps its history)" : ""}`).join(", ") }
      : { ok: "bad", text: "No agents yet", detail: "Add an agent to start." },
  );
  out.push(
    w.edges.length
      ? { ok: "done", text: `${w.edges.length} connection${w.edges.length === 1 ? "" : "s"}`, detail: w.edges.map((e) => `${nodeName(v, e.from)} → ${nodeName(v, e.to)} · ${POLICY[e.policy].word.toLowerCase()}`).join("; ") }
      : { ok: w.nodes.length > 1 ? "warn" : "info", text: "No connections", detail: "Drag from an agent's dot to another agent to let it hand work over." },
  );
  out.push({ ok: "info", text: "Wasl connector will be added to every agent", detail: "It lets them hand off work to each other. Their own CLI config stays as it is." });
  for (const h of hosts) if (!hostUp(h)) out.push({ ok: "warn", text: `${hostName(h)} is not connected`, detail: "Its agents launch once it is back." });
  for (const n of w.nodes) if (!n.session && !n.launch?.first_prompt) out.push({ ok: "warn", text: `${n.name} has no first task`, detail: "That is fine. It waits until another agent hands it something." });
  return out;
}

export const launchable = (n: WorkstreamNode) => !n.session && !!n.launch;
