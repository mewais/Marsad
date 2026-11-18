import type { GateRequest, PermissionChoice, PlanEntry, SessionEvent, SessionSnapshot, Usage } from "../types";
import { DRIVER_CATALOG } from "./catalog";
import { buildTranscript } from "./transcripts";

/** Fixture-only: the ~14 sessions of the mockup, as compact specs expanded into wire snapshots. */

export type SpecKind = "needs" | "failed" | "done" | "working" | "idle";

export interface Spec {
  n: number;
  name: string;
  driver: string;
  host: string;
  proj: string;
  branch: string;
  ws?: string;
  /** Minutes since the last activity. */
  agoMin: number;
  cost: number;
  kind: SpecKind;
  line: string;
  /** Files, added, removed (finished sessions). */
  stat?: [number, number, number];
  gates?: GateRequest[];
  /** Fixture: the session has a long history (paging demo). */
  longHistory?: boolean;
}

const claudeChoices = (rule: string): PermissionChoice[] => [
  { id: "allow", label: "Yes", kind: "allow" },
  { id: "always", label: `Yes, and don't ask again for \`${rule}\` commands in this project`, kind: "always" },
  { id: "deny", label: "No, and tell Claude what to do differently", kind: "deny", asks_reason: true },
];

export const SPECS: Spec[] = [
  { n: 7, name: "migrate-auth-middleware", driver: "claude", host: "x1", proj: "ledgerline", branch: "refactor/auth-mw", agoMin: 1, cost: 2.41, kind: "needs",
    line: "Asks: how should legacy session cookies be handled during rollout?",
    gates: [
      { gate_type: "question", request_id: "q-cookies", questions: [{
        header: "Legacy cookies", multi_select: false,
        question: "How should legacy session cookies be handled during rollout?",
        options: [
          { label: "Accept legacy cookie for 30 days", description: "No forced logouts; compat code is deleted after the window. (Recommended)" },
          { label: "Migrate on first request", description: "Reads connect.sid, issues the new token, deletes the old key. Adds ~4ms to the first request." },
          { label: "Invalidate all at deploy", description: "Simplest. About 41k users get signed out once." },
        ] }] },
      { gate_type: "permission", request_id: "p-prisma", tool_name: "Bash", command: "pnpm prisma migrate dev --name session_tokens", cwd: "~/src/ledgerline",
        explanation: "Writes to a database. Creates prisma/migrations/20261117_session_tokens and applies it to postgres://localhost:5432/ledgerline_dev. Reversible with migrate reset; dev data would be lost.",
        permission_suggestions: [{ type: "addRules", rules: [{ toolName: "Bash", ruleContent: "pnpm prisma migrate:*" }], behavior: "allow", destination: "localSettings" }],
        choices: claudeChoices("pnpm prisma migrate") },
    ] },
  { n: 2, name: "pdf-renderer-worker", driver: "claude", host: "hz", proj: "ledgerline", branch: "feat/invoice-pdf", ws: "Invoice export v2", agoMin: 3, cost: 1.12, kind: "needs",
    line: "Wants to run pnpm add @react-pdf/renderer@4.1.0",
    gates: [{ gate_type: "permission", request_id: "p-pdf", tool_name: "Bash", command: "pnpm add @react-pdf/renderer@4.1.0 --filter @ledgerline/export", cwd: "~/src/ledgerline",
      choices: claudeChoices("pnpm add") }] },
  { n: 15, name: "ws-reconnect-backoff", driver: "codex", host: "hz", proj: "marsad", branch: "fix/ws-backoff", agoMin: 4, cost: 0.58, kind: "needs",
    line: "Wants to run cargo test -p marsad-net --features e2e",
    gates: [{ gate_type: "permission", request_id: "p-cargo", tool_name: "shell", command: "cargo test -p marsad-net --features e2e", cwd: "~/src/marsad",
      choices: [{ id: "allow", label: "Approve", kind: "allow" }, { id: "always", label: "Approve for this session", kind: "always" }, { id: "deny", label: "Deny", kind: "deny" }] }] },
  { n: 4, name: "windows-print-check", driver: "claude", host: "win", proj: "ledgerline", branch: "feat/invoice-export-v2", ws: "Invoice export v2", agoMin: 6, cost: 0.37, kind: "needs",
    line: "Handoff waiting: orchestrator wants to delegate a task",
    gates: [{ gate_type: "handoff", request_id: "h-print", from_name: "invoice-export-orchestrator", to_name: "windows-print-check",
      task: "Verify the generated invoice PDFs print correctly through the Windows print spooler at A4 and Letter.",
      payload: "Context: apps/export/out/*.pdf (14 files). Expected: no scaling, 10mm margins. Report back with a pass/fail table." }] },
  { n: 5, name: "ci-triage-lead", driver: "claude", host: "hz", proj: "marsad", branch: "fix/flaky-ssh-test", ws: "Flaky CI triage", agoMin: 12, cost: 1.96, kind: "failed",
    line: "pnpm test exited 1 · 3 failing in ssh-tunnel.spec.ts (timeout 5000ms)" },
  { n: 11, name: "msi-installer", driver: "kilo", host: "win", proj: "marsad", branch: "feat/msi-installer", agoMin: 38, cost: 0.84, kind: "failed",
    line: "WiX build failed: error ICE03 invalid language id 1025 in Product.wxs:14" },
  { n: 6, name: "ci-repro-windows", driver: "kilo", host: "win", proj: "marsad", branch: "fix/flaky-ssh-test", ws: "Flaky CI triage", agoMin: 9, cost: 0.52, kind: "done",
    line: "Reproduced on Windows: named-pipe handshake races the 5s timeout. Report attached.", stat: [2, 41, 6] },
  { n: 8, name: "tile-cache-eviction", driver: "codex", host: "hz", proj: "tilemap-renderer", branch: "perf/lru-eviction", agoMin: 21, cost: 0.88, kind: "done",
    line: "LRU eviction now bounded by bytes, not entries. p95 render 212ms → 131ms; 48 tests pass.", stat: [5, 212, 38] },
  { n: 13, name: "rename-api-v1-routes", driver: "opencode", host: "hz", proj: "ledgerline", branch: "chore/routes-v1", agoMin: 47, cost: 0.31, kind: "done",
    line: "Renamed 23 routes under /v1, kept 301 redirects for the old paths. OpenAPI spec regenerated.", stat: [11, 168, 151] },
  { n: 1, name: "invoice-export-orchestrator", driver: "claude", host: "x1", proj: "ledgerline", branch: "feat/invoice-export-v2", ws: "Invoice export v2", agoMin: 0, cost: 3.84, kind: "working",
    line: "Waiting on 2 of 3 delegated tasks · merging CSV schema from csv-schema-worker" },
  { n: 3, name: "csv-schema-worker", driver: "opencode", host: "hz", proj: "ledgerline", branch: "feat/invoice-csv", ws: "Invoice export v2", agoMin: 0, cost: 0.95, kind: "working",
    line: "Running vitest on packages/export (34/52 passed so far)" },
  { n: 10, name: "upgrade-tauri-2-5", driver: "claude", host: "x1", proj: "marsad", branch: "chore/tauri-2.5", agoMin: 0, cost: 1.27, kind: "working",
    line: "Editing src-tauri/capabilities/default.json for the new permission schema", longHistory: true },
  { n: 9, name: "docs-search-index", driver: "opencode", host: "x1", proj: "docs-site", branch: "main", agoMin: 120, cost: 0.44, kind: "idle", line: "Finished yesterday 18:10 · seen" },
  { n: 12, name: "sqlite-wal-tuning", driver: "codex", host: "hz", proj: "ledgerline", branch: "perf/wal", agoMin: 300, cost: 0.62, kind: "idle", line: "Paused by you at 10:32" },
  { n: 14, name: "release-notes-1-4", driver: "claude", host: "x1", proj: "marsad", branch: "release/1.4", agoMin: 1440, cost: 0.33, kind: "idle", line: "Draft ready in CHANGELOG.md · seen" },
];

const ALPHABET = "abcdefghijklmnopqrstuvwxyz234567";
/** Deterministic 12-char base32 id (valid per wire.rs SessionId). */
export function fixtureId(n: number): string {
  let s = "";
  let v = n * 7919 + 31;
  for (let i = 0; i < 8; i++) {
    s = ALPHABET[v % 32] + s;
    v = Math.floor(v / 32) + n * 13 + i;
  }
  return "fx" + s + ALPHABET[n % 32] + ALPHABET[(n * 3) % 32];
}

const usage = (cost: number): Usage => ({
  input_tokens: Math.round(cost * 170_000),
  output_tokens: Math.round(cost * 15_000),
  cache_read_tokens: Math.round(cost * 120_000),
  cache_creation_tokens: Math.round(cost * 9_000),
  cost_usd: cost,
});

export interface Built {
  snap: SessionSnapshot;
  events: SessionEvent[];
}

export function buildSession(spec: Spec, now: number): Built {
  const drv = DRIVER_CATALOG.find((d) => d.id === spec.driver)!;
  const id = fixtureId(spec.n);
  const last = now - spec.agoMin * 60_000;
  const { events, pending, plan } = buildTranscript(spec, id, now);
  const phase: SessionSnapshot["phase"] =
    spec.kind === "needs" ? { state: "needs_you" }
    : spec.kind === "failed" ? { state: "failed", reason: spec.line, stderr_tail: [spec.line] }
    : spec.kind === "working" ? { state: "working" }
    : { state: "idle" };
  const finishedAt = spec.kind === "done" || spec.kind === "idle" ? last : undefined;
  const planEntries: PlanEntry[] = plan;
  const snap: SessionSnapshot = {
    id,
    driver: spec.driver,
    cwd: `~/src/${spec.proj}`,
    labels: {
      name: spec.name,
      "git.branch": spec.branch,
      ...(spec.ws ? { workstream: spec.ws } : {}),
      ...(spec.stat ? { "diff.files": String(spec.stat[0]), "diff.added": String(spec.stat[1]), "diff.removed": String(spec.stat[2]) } : {}),
    },
    created_at_ms: now - 3_600_000 * 2,
    capabilities: drv.capabilities,
    phase,
    generation: 1,
    pid: 1000 + spec.n,
    native_session_id: `native-${spec.n}`,
    title: spec.name,
    cli_version: drv.version,
    model: drv.models[0].id,
    mode: drv.modes[0].id,
    available_models: drv.models,
    available_modes: drv.modes,
    slash_commands: drv.slash,
    pending_gates: pending,
    turn: spec.kind === "working" || spec.kind === "needs"
      ? { started_at_ms: now - 600_000, prompt: spec.name, tool_calls: 9 + spec.n, last_tool: spec.kind === "working" ? "Edit" : "Bash" }
      : undefined,
    last_turn: finishedAt
      ? { ended_at_ms: finishedAt, was_interrupted: false, num_turns: 3, reply: spec.line }
      : spec.kind === "failed" ? { ended_at_ms: last, was_interrupted: false, num_turns: 2, end_reason: "execution_error", reply: spec.line } : undefined,
    turns_completed: spec.kind === "working" ? 1 : 3,
    usage: usage(spec.cost),
    context_pct: Math.min(90, 12 + spec.n * 3),
    subagents: spec.n === 7 ? [{ call_id: "c-sub", name: "Explore", description: "List routes that depend on session shape", started_at_ms: now - 500_000, running: false, ok: true }] : [],
    plan: spec.kind === "working" ? [...planEntries.slice(0, 1), { content: spec.line, priority: "medium", status: "in_progress" }] : planEntries,
    last_activity_ms: last,
    exit: undefined,
    stderr_tail: spec.kind === "failed" ? [spec.line] : [],
    cursor: events[events.length - 1]?.cursor ?? 0,
    loaded: true,
  };
  return { snap, events };
}
