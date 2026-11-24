import type { GateChoice, GateRequest, PlanEntry, SessionEvent, SessionSnapshot, Usage } from "../wire";
import { type ConversationFacts, conversationFacts, driverDef } from "./catalog";
import { NO_EARLIER, type BuiltEarlier, type EarlierKind } from "./earlier";
import { buildTranscript, modelSplit } from "./transcripts";
import { foldWork } from "./work";

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
  /** Fixture: how many scripted work rounds the generic transcript runs (thousands-of-rows demo); overrides the default. */
  rounds?: number;
  /** Fixture: it is working only because tasks run in the background (no turn is open). */
  backgroundOnly?: boolean;
  /** Fixture: the session resumed a conversation, and its earlier history is complete, still arriving, or cannot be had (see `earlier.ts`). */
  earlier?: EarlierKind;
}

/**
 * What wasit-claude offers for a `can_use_tool` request (neutral labels; see wasit-core `GateChoice`).
 * `remember` is the choice's description; `effects` is what "Allow and remember" applies, one per suggestion.
 */
export function claudeChoices(remember?: string, effects: string[] = []): GateChoice[] {
  return [
    { id: "allow", label: "Allow", kind: "allow_once", neutral_label: true },
    ...(remember ? [{ id: "allow_always", label: "Allow and remember", kind: "allow_always" as const, neutral_label: true, description: remember, effects }] : []),
    { id: "deny", label: "Deny", kind: "reject_once", neutral_label: true, text: "optional" },
    { id: "deny_stop", label: "Deny and stop the turn", kind: "cancel", neutral_label: true, text: "optional" },
  ];
}
/** codex `availableDecisions` for a command (accept, remember the prefix, cancel). */
export const CODEX_CHOICES: GateChoice[] = [
  { id: "accept", label: "Allow", kind: "allow_once", neutral_label: true },
  { id: "acceptWithExecpolicyAmendment", label: "Allow and remember", kind: "allow_always", neutral_label: true, description: "commands starting with `cargo test`" },
  { id: "cancel", label: "Deny and stop the turn", kind: "cancel", neutral_label: true },
];
/** wasl-proto `handoff_choices()`. */
export const HANDOFF_CHOICES: GateChoice[] = [
  { id: "allow_once", label: "Allow once", kind: "allow_once" },
  { id: "always_allow", label: "Always allow on this connection", kind: "allow_always", description: "sets the connection's policy to always allow" },
  { id: "deny", label: "Deny", kind: "reject_once", text: "optional" },
];

/** wasl-proto `spawn_choices()`. */
export const SPAWN_CHOICES: GateChoice[] = [
  { id: "allow_once", label: "Allow once", kind: "allow_once" },
  { id: "always_allow_spawning", label: "Always allow spawning in this workstream", kind: "allow_always", description: "sets the workstream's spawn policy to always allow; handoffs to spawned agents still ask" },
  { id: "always_allow_spawning_and_handoffs", label: "Always allow spawning and handoffs to spawned agents", kind: "allow_always", description: "also lets this workstream's agents hand work to the agents they spawn without asking" },
  { id: "deny", label: "Deny", kind: "reject_once", text: "optional" },
];

/** The first session of a project on a host works in the checkout; later ones in worktrees (one branch per folder, as in git). */
export function fixtureCwd(spec: Spec, specs: Spec[] = SPECS): string {
  const first = specs.find((x) => x.host === spec.host && x.proj === spec.proj);
  return !first || first === spec || first.n === spec.n ? `~/src/${spec.proj}` : `~/src/${spec.proj}-worktrees/${spec.branch.split("/").pop()}`;
}

export const SPECS: Spec[] = [
  { n: 7, name: "migrate-auth-middleware", driver: "claude", host: "x1", proj: "ledgerline", branch: "refactor/auth-mw", agoMin: 1, cost: 2.41, kind: "needs",
    line: "Asks: how should legacy session cookies be handled during rollout?",
    gates: [
      { gate_type: "question", request_id: "q-cookies", questions: [{
        id: "How should legacy session cookies be handled during rollout?", header: "Legacy cookies", multi_select: false, allow_other: true,
        question: "How should legacy session cookies be handled during rollout?",
        options: [
          { label: "Accept legacy cookie for 30 days", description: "No forced logouts; compat code is deleted after the window. (Recommended)" },
          { label: "Migrate on first request", description: "Reads connect.sid, issues the new token, deletes the old key. Adds ~4ms to the first request." },
          { label: "Invalidate all at deploy", description: "Simplest. About 41k users get signed out once." },
        ] }] },
      { gate_type: "permission", request_id: "p-prisma", tool_name: "Bash", command: "pnpm prisma migrate dev --name session_tokens", cwd: "~/src/ledgerline",
        explanation: "Writes to a database. Creates prisma/migrations/20261117_session_tokens and applies it to postgres://localhost:5432/ledgerline_dev. Reversible with migrate reset; dev data would be lost.",
        choices: claudeChoices("allow rule Bash(pnpm prisma migrate:*) (localSettings)", ["allow rule Bash(pnpm prisma migrate:*) (localSettings)", "add directory ~/src/ledgerline (session)"]), raw_input: { command: "pnpm prisma migrate dev --name session_tokens" } },
    ] },
  { n: 2, name: "pdf-renderer-worker", driver: "claude", host: "hz", proj: "ledgerline", branch: "feat/invoice-pdf", ws: "Invoice export v2", agoMin: 3, cost: 1.12, kind: "needs",
    line: "Wants to run pnpm add @react-pdf/renderer@4.1.0",
    gates: [{ gate_type: "permission", request_id: "p-pdf", tool_name: "Bash", command: "pnpm add @react-pdf/renderer@4.1.0 --filter @ledgerline/export", cwd: "~/src/ledgerline", explanation: "",
      choices: claudeChoices("allow rule Bash(pnpm add:*) (localSettings)", ["allow rule Bash(pnpm add:*) (localSettings)", "switch to mode acceptEdits (session)"]), raw_input: { command: "pnpm add @react-pdf/renderer@4.1.0 --filter @ledgerline/export" } }] },
  { n: 15, name: "ws-reconnect-backoff", driver: "codex", host: "hz", proj: "marsad", branch: "fix/ws-backoff", agoMin: 4, cost: 0.58, kind: "needs",
    line: "Wants to run cargo test -p marsad-net --features e2e",
    gates: [{ gate_type: "permission", request_id: "p-cargo", tool_name: "shell", command: "cargo test -p marsad-net --features e2e", cwd: "~/src/marsad", explanation: "",
      choices: CODEX_CHOICES, raw_input: { command: ["cargo", "test", "-p", "marsad-net", "--features", "e2e"] } }] },
  { n: 4, name: "windows-print-check", driver: "claude", host: "win", proj: "ledgerline", branch: "feat/invoice-export-v2", ws: "Invoice export v2", agoMin: 75, cost: 0.37, kind: "idle",
    line: "Not started · waiting for a handoff from invoice-export-orchestrator" },
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
  { n: 1, name: "invoice-export-orchestrator", driver: "claude", host: "x1", proj: "ledgerline", branch: "feat/invoice-export-v2", ws: "Invoice export v2", agoMin: 6, cost: 3.84, kind: "needs",
    line: "Handoff waiting: delegate print verification to windows-print-check",
    gates: [{ gate_type: "handoff", request_id: "wasl-handoff-t4print", task_id: "t4print", from: "thinkpad-x1/invoice-export-orchestrator", to: "win-build/windows-print-check", mode: "command",
      instruction: "Verify the generated invoice PDFs print correctly through the Windows print spooler at A4 and Letter. Report any clipping of the totals table.",
      details: { attachments: ["out/invoice-INV-2041.pdf", "out/invoice-INV-2042.pdf"], hop: 1, budget_usd: 1.5 }, choices: HANDOFF_CHOICES }] },
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
  /** The conversation's earlier history (history items, apart from the events) and what is still to arrive of it. */
  earlier: SessionEvent[];
  earlierPlan?: BuiltEarlier["plan"];
}

/** How the last turn in `events` ended (the snapshot's `last_turn`). */
function lastTurnOf(events: SessionEvent[]): SessionSnapshot["last_turn"] {
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i];
    if (e.kind === "agent" && e.event.type === "turn_completed") return { ended_at_ms: e.t, was_interrupted: e.event.was_interrupted, num_turns: e.event.num_turns, reply: e.event.reply, origin: e.event.origin };
  }
  return undefined;
}

export function buildSession(spec: Spec, now: number): Built {
  const drv = driverDef(spec.driver);
  const id = fixtureId(spec.n);
  const last = now - spec.agoMin * 60_000;
  const { events, pending, plan, earlier } = buildTranscript(spec, id, now);
  // What the session's events say about its subagents and background tasks, as the daemon's snapshot would.
  const work = foldWork(events);
  const phase: SessionSnapshot["phase"] =
    spec.kind === "needs" ? { state: "needs_you" }
    : spec.kind === "failed" ? { state: "failed", reason: spec.line, stderr_tail: [spec.line] }
    : spec.kind === "working" ? { state: "working" }
    : { state: "idle" };
  const finishedAt = spec.kind === "done" || spec.kind === "idle" ? last : undefined;
  const planEntries: PlanEntry[] = plan;
  const facts: Omit<SessionSnapshot, keyof ConversationFacts> = {
    id,
    driver: spec.driver,
    cwd: fixtureCwd(spec),
    labels: {},
    created_at_ms: now - 3_600_000 * 2,
    capabilities: drv.capabilities,
    prompt_input: drv.prompt_input,
    phase,
    generation: 1,
    pid: 1000 + spec.n,
    native_session_id: `native-${spec.n}`,
    title: spec.name,
    cli_version: drv.version,
    model: drv.models[spec.n === 12 ? 1 : 0].id,
    mode: drv.modes[0].id,
    available_models: drv.models,
    available_modes: drv.modes,
    slash_commands: drv.slash,
    pending_gates: pending,
    turn: !spec.backgroundOnly && (spec.kind === "working" || spec.kind === "needs")
      ? { started_at_ms: now - 600_000, trigger: "input" as const, prompt: spec.name, tool_calls: 9 + spec.n, last_tool: spec.kind === "working" ? "Edit" : "Bash" }
      : undefined,
    last_turn: spec.backgroundOnly ? lastTurnOf(events)
      : finishedAt
      ? { ended_at_ms: finishedAt, was_interrupted: false, num_turns: 3, reply: spec.line }
      : spec.kind === "failed" ? { ended_at_ms: last, was_interrupted: false, num_turns: 2, end_reason: "execution_error", reply: spec.line } : undefined,
    turns_completed: spec.kind === "working" ? 1 : 3,
    usage: usage(spec.cost),
    usage_by_model: modelSplit(spec.driver, usage(spec.cost), spec.cost),
    context_pct: Math.min(90, 12 + spec.n * 3),
    ...work,
    plan: spec.kind === "working" && !spec.backgroundOnly ? [...planEntries.slice(0, 1), { content: spec.line, priority: "medium", status: "in_progress" }] : planEntries,
    last_activity_ms: last,
    exit: undefined,
    stderr_tail: spec.kind === "failed" ? [spec.line] : [],
    cursor: events[events.length - 1]?.cursor ?? 0,
    loaded: true,
    earlier: earlier?.info ?? NO_EARLIER,
  };
  return { snap: { ...facts, ...conversationFacts(facts) }, events, earlier: earlier?.items ?? [], earlierPlan: earlier?.plan };
}
