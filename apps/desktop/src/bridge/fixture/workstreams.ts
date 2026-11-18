import type {
  AddNode, Connect, EdgeHistoryRequest, EdgeRef, EdgeView, FromTemplate, HandoffGateView, LaunchReply, LaunchRequest, NodeRef, SaveTemplate, SessionSnapshot,
  TaskView, TemplateRef, Workstream, WorkstreamDraft, WorkstreamNode, WorkstreamRef, WorkstreamTemplate, WorkstreamUpdate, WorkstreamView, WorkstreamsEvent,
} from "../wire";
import { methods as M } from "../wire";
import { err } from "./runtime";
import { fixtureId } from "./sessions";

/** Fixture-only: the hub's workstream service (documents, live views, tasks, templates). */

export interface WsCore {
  now(): number;
  session(host: string, id: string): SessionSnapshot | undefined;
  launch(r: LaunchRequest): LaunchReply;
  emit(e: WorkstreamsEvent): void;
}

const TERMINAL = new Set(["completed", "failed", "cancelled", "denied", "expired"]);
const limits = { max_depth: 4, default_deadline_secs: 1800, budget_usd: null };
const node = (id: string, name: string, host: string, n: number | null, launch?: WorkstreamNode["launch"]): WorkstreamNode =>
  ({ id, name, role: "", description: "", session: n == null ? null : { host, session: fixtureId(n) }, launch: launch ?? null });

function seedDocs(now: number): Workstream[] {
  const base = { schema: 1, limits, topology_hosts: [], template: null, created_at_ms: now - 4 * 3_600_000 };
  return [
    { ...base, id: "ws-invoice", name: "Invoice export v2", description: "CSV and PDF invoice export, checked on the Windows print spooler", state: "running", topology_version: 3, revision: 7, updated_at_ms: now - 50 * 60_000,
      nodes: [node("orchestrator", "invoice-export-orchestrator", "x1", 1), node("pdf", "pdf-renderer-worker", "hz", 2), node("csv", "csv-schema-worker", "hz", 3), node("print", "windows-print-check", "win", 4)],
      edges: [{ from: "orchestrator", to: "pdf", policy: "ask" }, { from: "orchestrator", to: "csv", policy: "always_allow" }, { from: "orchestrator", to: "print", policy: "ask" },
        { from: "csv", to: "pdf", policy: "always_allow" }, { from: "print", to: "pdf", policy: "deny" }], layout: {} },
    { ...base, id: "ws-ci", name: "Flaky CI triage", description: "Find why ssh-tunnel.spec.ts times out on CI", state: "running", topology_version: 1, revision: 3, updated_at_ms: now - 170 * 60_000,
      nodes: [node("lead", "ci-triage-lead", "hz", 5), node("repro", "ci-repro-windows", "win", 6)], edges: [{ from: "lead", to: "repro", policy: "always_allow" }], layout: {} },
    { ...base, id: "ws-docs", name: "Docs search revamp", description: "Rebuild the docs search index and wire it into the site header", state: "draft", topology_version: 0, revision: 2, updated_at_ms: now - 330 * 60_000,
      nodes: [node("index", "docs-search-index", "x1", 9), node("lead", "search-lead", "hz", null, { host: "hz", driver: "claude", cwd: "~/src/docs-site", model: "claude-sonnet-4.5", mode: null, first_prompt: "Plan the search revamp, hand indexing to docs-search-index." })],
      edges: [{ from: "lead", to: "index", policy: "ask" }], layout: {} },
  ];
}

function seedTemplates(now: number): WorkstreamTemplate[] {
  const t = (id: string, name: string, description: string, nodes: WorkstreamNode[], edges: WorkstreamTemplate["edges"]): WorkstreamTemplate => ({ id, schema: 1, name, description, nodes, edges, limits, layout: {}, created_at_ms: now - 9 * 86_400_000 });
  const l = (driver: string, host = "x1") => ({ host, driver, cwd: "~/src/project" });
  return [
    t("tpl-orch", "Orchestrator + 2 workers", "1 Claude Code lead, 2 workers on any hosts · ask each time", [node("lead", "lead", "x1", null, l("claude")), node("w1", "worker-1", "hz", null, l("codex", "hz")), node("w2", "worker-2", "hz", null, l("opencode", "hz"))],
      [{ from: "lead", to: "w1", policy: "ask" }, { from: "lead", to: "w2", policy: "ask" }]),
    t("tpl-os", "Linux build + Windows check", "2 agents, one per OS · always allow", [node("build", "linux-build", "hz", null, l("claude", "hz")), node("check", "windows-check", "win", null, l("claude", "win"))], [{ from: "build", to: "check", policy: "always_allow" }]),
    t("tpl-review", "Writer and reviewer", "Codex writes, Claude Code reviews · ask each time", [node("writer", "writer", "x1", null, l("codex")), node("reviewer", "reviewer", "x1", null, l("claude"))], [{ from: "writer", to: "reviewer", policy: "ask" }]),
  ];
}

function seedTasks(now: number): TaskView[] {
  const min = 60_000;
  const a = (host: string, n: number) => `${host}/${fixtureId(n)}`;
  const orch = a("x1", 1);
  const t = (id: string, from: string, to: string, instruction: string, state: TaskView["state"], ago: number, workstream = "ws-invoice"): TaskView => ({
    task: { id, workstream, from, to, mode: "command", instruction, deadline_ms: now + 30 * min, created_ms: now - ago * min, chain: [from], hop: 1, attachments: [] }, state, updated_ms: now - Math.max(0, ago - 2) * min,
  });
  const done = (summary: string, cost: number): TaskView["state"] => ({ state: "completed", result: { status: "success", summary, artifacts: [], usage: { input_tokens: Math.round(cost * 180_000), output_tokens: Math.round(cost * 14_000), cache_read_tokens: 0, cache_creation_tokens: 0, cost_usd: cost } } });
  return [
    t("t-pdf", orch, a("hz", 2), "Render invoice PDF template", { state: "running", progress: { tool_calls: 14, last_tool: "Bash", last_text: "Blocked on your approval" } }, 22),
    t("t-csv", orch, a("hz", 3), "Generate CSV schema + fixtures", { state: "running", progress: { tool_calls: 31, last_tool: "vitest", last_text: "34/52 tests passed" } }, 30),
    t("t4print", orch, a("win", 4), "Verify printing via spooler", { state: "pending_approval", via: "gate" }, 6),
    t("t-gs", orch, a("win", 4), "Install Ghostscript system-wide", { state: "denied", reason: "use the bundled pdfium instead" }, 51),
    t("t-font", orch, a("win", 4), "Check Windows font fallback for Arabic invoice lines", done("Fallback to Segoe UI works; no missing glyphs.", 0.22), 90),
    t("t-clone", orch, a("win", 4), "Clone repo and install deps", done("Cloned and installed.", 0.06), 150),
    t("t-peer", a("hz", 3), a("hz", 2), "Share the CSV column order", done("Sent the column order.", 0.02), 12),
    t("t-rep", a("hz", 5), a("win", 6), "Reproduce the ssh-tunnel timeout on Windows", done("Reproduced: named-pipe handshake races the 5s timeout.", 0.41), 40, "ws-ci"),
  ];
}

export class FixtureWorkstreams {
  docs = new Map<string, Workstream>();
  templates: WorkstreamTemplate[];
  tasks: TaskView[];

  constructor(private core: WsCore) {
    const now = core.now();
    for (const d of seedDocs(now)) this.docs.set(d.id, d);
    this.templates = seedTemplates(now);
    this.tasks = seedTasks(now);
  }

  private aid(w: Workstream, nodeId: string): string | null {
    const s = w.nodes.find((n) => n.id === nodeId)?.session;
    return s ? `${s.host}/${s.session}` : null;
  }

  view(w: Workstream): WorkstreamView {
    const nodes = w.nodes.map((n) => {
      const host = n.session?.host ?? n.launch?.host ?? "x1";
      return { node: n.id, host, session: n.session ? this.core.session(n.session.host, n.session.session) ?? null : null, launching: false };
    });
    const edges: EdgeView[] = w.edges.map((e) => {
      const from = this.aid(w, e.from);
      const to = this.aid(w, e.to);
      const mine = this.tasks.filter((t) => t.task.workstream === w.id && t.task.from === from && t.task.to === to);
      const sender = w.nodes.find((n) => n.id === e.from)?.session;
      const snap = sender ? this.core.session(sender.host, sender.session) : undefined;
      const gates: HandoffGateView[] = (snap?.pending_gates ?? [])
        .filter((g) => g.gate.gate_type === "handoff" && mine.some((t) => t.task.id === (g.gate as { task_id: string }).task_id))
        .map((g) => ({ host: sender!.host, session: sender!.session, task: (g.gate as { task_id: string }).task_id, raised_at_ms: g.raised_at_ms, gate: g.gate }));
      const ok = mine.filter((t) => t.state.state === "completed");
      return {
        from: e.from, to: e.to, policy: e.policy,
        in_flight: mine.filter((t) => !TERMINAL.has(t.state.state)), pending_gates: gates,
        completed: ok.length, unsuccessful: mine.filter((t) => TERMINAL.has(t.state.state) && t.state.state !== "completed").length,
        cost_usd: ok.reduce((c, t) => c + (t.state.state === "completed" ? t.state.result.usage?.cost_usd ?? 0 : 0), 0),
      };
    });
    const by_node: Record<string, number> = {};
    for (const n of nodes) by_node[n.node] = n.session?.usage.cost_usd ?? 0;
    return {
      workstream: w, nodes, edges,
      sync: [...new Set(nodes.map((n) => n.host))].map((host) => ({ host, applied: w.state === "draft" ? 0 : w.topology_version, error: null })),
      cost: { sessions_usd: Object.values(by_node).reduce((a, b) => a + b, 0), tasks_usd: edges.reduce((a, e) => a + e.cost_usd, 0), by_node },
    };
  }

  reset(): WorkstreamsEvent {
    return { kind: "reset", workstreams: [...this.docs.values()].map((w) => this.view(w)) };
  }

  private put(w: Workstream): Workstream {
    const next = { ...w, revision: w.revision + 1, updated_at_ms: this.core.now() };
    this.docs.set(w.id, next);
    this.core.emit({ kind: "upserted", workstream: this.view(next) });
    return next;
  }

  private get(id: string): Workstream {
    const w = this.docs.get(id);
    if (!w) throw err("not_found", `No workstream ${id}`, { marsad_error: "not_found", what: `workstream ${id}` });
    return w;
  }

  /** A session changed: refresh the views that contain it. */
  onSession(host: string, id: string) {
    for (const w of this.docs.values()) if (w.nodes.some((n) => n.session?.host === host && n.session.session === id)) this.core.emit({ kind: "upserted", workstream: this.view(w) });
  }

  /** A handoff gate was answered on its sending session. */
  onHandoffAnswer(requestId: string, denied: boolean) {
    const t = this.tasks.find((x) => `wasl-handoff-${x.task.id}` === requestId);
    if (!t) return;
    t.state = denied ? { state: "denied", reason: "denied by you" } : { state: "queued" };
    t.updated_ms = this.core.now();
    const w = this.docs.get(t.task.workstream);
    if (w) this.core.emit({ kind: "upserted", workstream: this.view(w) });
  }

  private launchNode(w: Workstream, n: WorkstreamNode): WorkstreamNode {
    if (n.session || !n.launch) return n;
    const l = n.launch;
    const r = this.core.launch({ host: l.host ?? "x1", driver: l.driver, cwd: l.cwd ?? null, model: l.model ?? null, mode: l.mode ?? null, first_prompt: l.first_prompt ?? null, resume: { kind: "new" }, labels: l.labels ?? {}, workstream: { workstream: w.id, node: n.id }, overrides: l.overrides ?? { items: [], env: [], disallowed_tools: [] } });
    return { ...n, session: { host: r.host, session: r.id } };
  }

  handle(method: string, p: unknown): unknown {
    const now = this.core.now();
    switch (method) {
      case M.WS_LIST:
        return { workstreams: [...this.docs.values()] };
      case M.WS_GET:
        return this.get((p as WorkstreamRef).id);
      case M.WS_CREATE: {
        const d = p as WorkstreamDraft;
        if (!d.name.trim()) throw err("invalid_params", "A workstream needs a name.");
        let id = d.id ?? `ws-${d.name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "")}`;
        while (this.docs.has(id)) id += "-2";
        const w: Workstream = { id, schema: 1, name: d.name, description: d.description, state: "draft", nodes: d.nodes, edges: d.edges, limits: d.limits, layout: d.layout, topology_version: 0, topology_hosts: [], template: null, revision: 0, created_at_ms: now, updated_at_ms: now };
        return this.put(w);
      }
      case M.WS_UPDATE: {
        const u = p as WorkstreamUpdate;
        const cur = this.get(u.workstream.id);
        if (u.expected_revision != null && u.expected_revision !== cur.revision) throw err("store", "Someone else changed this workstream. Reload and try again.");
        const sessions = new Map(cur.nodes.map((n) => [n.id, n.session]));
        return this.put({ ...cur, name: u.workstream.name, description: u.workstream.description, limits: u.workstream.limits, layout: u.workstream.layout, edges: u.workstream.edges, nodes: u.workstream.nodes.map((n) => ({ ...n, session: sessions.get(n.id) ?? n.session ?? null })) });
      }
      case M.WS_DELETE:
        this.get((p as WorkstreamRef).id);
        this.docs.delete((p as WorkstreamRef).id);
        this.core.emit({ kind: "removed", id: (p as WorkstreamRef).id });
        return {};
      case M.WS_LAUNCH: {
        const w = this.get((p as WorkstreamRef).id);
        const launched: { node: string; session?: { host: string; session: string } | null; error?: never }[] = [];
        const nodes = w.nodes.map((n) => {
          const m = this.launchNode(w, n);
          if (m !== n) launched.push({ node: n.id, session: m.session });
          return m;
        });
        return { workstream: this.put({ ...w, nodes, state: "running", topology_version: w.topology_version + 1 }), launched };
      }
      case M.WS_ADD_NODE: {
        const r = p as AddNode;
        const w = this.get(r.workstream);
        if (w.nodes.some((n) => n.id === r.node.id || n.name === r.node.name)) throw err("invalid_params", `A node named “${r.node.name}” is already in this workstream.`);
        const n = w.state === "running" ? this.launchNode(w, r.node) : r.node;
        return this.put({ ...w, nodes: [...w.nodes, n], layout: r.position ? { ...w.layout, [n.id]: r.position } : w.layout });
      }
      case M.WS_REMOVE_NODE: {
        const r = p as NodeRef;
        const w = this.get(r.workstream);
        return this.put({ ...w, nodes: w.nodes.filter((n) => n.id !== r.node), edges: w.edges.filter((e) => e.from !== r.node && e.to !== r.node) });
      }
      case M.WS_CONNECT:
      case M.WS_SET_POLICY: {
        const r = p as Connect;
        const w = this.get(r.workstream);
        if (r.from === r.to) throw err("invalid_params", "An agent cannot hand work to itself.");
        const exists = w.edges.some((e) => e.from === r.from && e.to === r.to);
        if (method === M.WS_SET_POLICY && !exists) throw err("not_found", "That connection does not exist.");
        const edges = exists ? w.edges.map((e) => (e.from === r.from && e.to === r.to ? { ...e, policy: r.policy } : e)) : [...w.edges, { from: r.from, to: r.to, policy: r.policy }];
        return this.put({ ...w, edges, topology_version: w.state === "draft" ? w.topology_version : w.topology_version + 1 });
      }
      case M.WS_DISCONNECT: {
        const r = p as EdgeRef;
        const w = this.get(r.workstream);
        return this.put({ ...w, edges: w.edges.filter((e) => !(e.from === r.from && e.to === r.to)) });
      }
      case M.WS_PAUSE:
      case M.WS_RESUME:
      case M.WS_ARCHIVE: {
        const w = this.get((p as WorkstreamRef).id);
        return this.put({ ...w, state: method === M.WS_PAUSE ? "paused" : method === M.WS_RESUME ? "running" : "archived" });
      }
      case M.WS_SAVE_TEMPLATE: {
        const r = p as SaveTemplate;
        const w = this.get(r.workstream);
        const t: WorkstreamTemplate = { id: `tpl-${w.id}-${this.templates.length}`, schema: 1, name: r.name ?? w.name, description: r.description ?? w.description, nodes: w.nodes.map((n) => ({ ...n, session: null })), edges: w.edges, limits: w.limits, layout: w.layout, created_at_ms: now };
        this.templates.push(t);
        return t;
      }
      case M.WS_FROM_TEMPLATE: {
        const r = p as FromTemplate;
        const t = this.templates.find((x) => x.id === r.template);
        if (!t) throw err("not_found", `No template ${r.template}`);
        const nodes = t.nodes.map((n) => ({ ...n, launch: n.launch ? { ...n.launch, host: r.hosts?.[n.launch.host ?? ""] ?? n.launch.host } : null }));
        return this.handle(M.WS_CREATE, { name: r.name ?? t.name, description: t.description ?? "", nodes, edges: t.edges, limits: t.limits, layout: t.layout ?? {} } satisfies WorkstreamDraft);
      }
      case M.WS_TEMPLATES_LIST:
        return { templates: this.templates };
      case M.WS_TEMPLATES_DELETE:
        this.templates = this.templates.filter((t) => t.id !== (p as TemplateRef).id);
        return {};
      case M.WS_HISTORY: {
        const r = p as EdgeHistoryRequest;
        const w = this.get(r.workstream);
        const from = this.aid(w, r.from);
        const to = this.aid(w, r.to);
        const tasks = this.tasks.filter((t) => t.task.workstream === w.id && t.task.from === from && t.task.to === to && (r.before_ms == null || t.task.created_ms < r.before_ms)).sort((a, b) => b.task.created_ms - a.task.created_ms);
        return { tasks: tasks.slice(0, r.limit ?? 50), next_before: null };
      }
      default:
        throw err("method_not_found", `Fixture does not implement ${method}`);
    }
  }
}
