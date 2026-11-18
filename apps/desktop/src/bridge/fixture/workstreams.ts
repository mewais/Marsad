import type { WorkstreamDoc } from "../wire";
import { WS_SCHEMA, type TaskView, type WsDocV1 } from "../provisional/workstreams";
import { fixtureId } from "./sessions";

/** Fixture-only: stored workstream documents and the Wasl tasks running in them. */

const sid = (n: number) => fixtureId(n);
const aid = (host: string, n: number) => `${host}/${sid(n)}`;

export function seedWorkstreams(now: number): WorkstreamDoc[] {
  const docs: [string, WsDocV1, number][] = [
    ["ws-invoice", {
      name: "Invoice export v2", description: "CSV and PDF invoice export, checked on the Windows print spooler", status: "launched",
      nodes: [
        { id: "orchestrator", name: "invoice-export-orchestrator", host: "x1", driver: "claude", cwd: "~/src/ledgerline", session: sid(1) },
        { id: "pdf", name: "pdf-renderer-worker", host: "hz", driver: "claude", cwd: "~/src/ledgerline", session: sid(2) },
        { id: "csv", name: "csv-schema-worker", host: "hz", driver: "opencode", cwd: "~/src/ledgerline", session: sid(3) },
        { id: "print", name: "windows-print-check", host: "win", driver: "claude", cwd: "C:\\src\\ledgerline", session: sid(4) },
      ],
      edges: [
        { id: "e-pdf", from: "orchestrator", to: "pdf", policy: "ask" },
        { id: "e-csv", from: "orchestrator", to: "csv", policy: "always_allow" },
        { id: "e-win", from: "orchestrator", to: "print", policy: "ask" },
        { id: "e-peer", from: "csv", to: "pdf", policy: "always_allow" },
        { id: "e-blk", from: "print", to: "pdf", policy: "deny" },
      ],
    }, 50],
    ["ws-ci", {
      name: "Flaky CI triage", description: "Find why ssh-tunnel.spec.ts times out on CI", status: "launched",
      nodes: [
        { id: "lead", name: "ci-triage-lead", host: "hz", driver: "claude", cwd: "~/src/marsad", session: sid(5) },
        { id: "repro", name: "ci-repro-windows", host: "win", driver: "kilo", cwd: "C:\\src\\marsad", session: sid(6) },
      ],
      edges: [{ id: "e-rep", from: "lead", to: "repro", policy: "always_allow" }],
    }, 170],
    ["ws-docs", {
      name: "Docs search revamp", description: "Rebuild the docs search index and wire it into the site header", status: "draft",
      nodes: [
        { id: "index", name: "docs-search-index", host: "x1", driver: "opencode", cwd: "~/src/docs-site", session: sid(9), resume: "native-9" },
        { id: "lead", name: "search-lead", host: "hz", driver: "claude", cwd: "~/src/docs-site", model: "claude-sonnet-4.5", first_prompt: "Plan the search revamp, hand indexing to docs-search-index." },
      ],
      edges: [{ id: "e-lead", from: "lead", to: "index", policy: "ask" }],
    }, 330],
  ];
  return docs.map(([id, doc, ago], i) => ({ id, schema: WS_SCHEMA, revision: 3 + i, updated_at_ms: now - ago * 60_000, doc: doc as never }));
}

export function seedTasks(now: number): TaskView[] {
  const min = 60_000;
  const t = (id: string, from: string, to: string, instruction: string, state: TaskView["state"], ago: number): TaskView => ({
    task: { id, workstream: "ws-invoice", from, to, mode: "command", instruction, deadline_ms: now + 30 * min, created_ms: now - ago * min, chain: [from], hop: 1 },
    state, updated_ms: now - Math.max(0, ago - 2) * min,
  });
  const orch = aid("x1", 1);
  return [
    t("t-pdf", orch, aid("hz", 2), "Render invoice PDF template", { state: "running", progress: { tool_calls: 14, last_tool: "Bash", last_text: "Blocked on your approval" } }, 22),
    t("t-csv", orch, aid("hz", 3), "Generate CSV schema + fixtures", { state: "running", progress: { tool_calls: 31, last_tool: "vitest", last_text: "34/52 tests passed" } }, 30),
    t("t4print", orch, aid("win", 4), "Verify printing via spooler", { state: "pending_approval", via: "gate" }, 6),
    t("t-gs", orch, aid("win", 4), "Install Ghostscript system-wide", { state: "denied", reason: "use the bundled pdfium instead" }, 51),
    t("t-font", orch, aid("win", 4), "Check Windows font fallback for Arabic invoice lines", { state: "completed", result: { status: "success", summary: "Fallback to Segoe UI works; no tofu.", usage: { input_tokens: 41_000, output_tokens: 3_100, cache_read_tokens: 0, cache_creation_tokens: 0, cost_usd: 0.22 } } }, 90),
    t("t-clone", orch, aid("win", 4), "Clone repo and install deps", { state: "completed", result: { status: "success", summary: "Cloned and installed.", usage: { input_tokens: 9_000, output_tokens: 800, cache_read_tokens: 0, cache_creation_tokens: 0, cost_usd: 0.06 } } }, 150),
    t("t-peer", aid("hz", 3), aid("hz", 2), "Share the CSV column order", { state: "completed", result: { status: "success", summary: "Sent." } }, 12),
  ];
}
