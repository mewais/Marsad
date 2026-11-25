import type { AgentEvent, Attachment, Scope, Command, ContentPart, JsonValue, ModelUsage, PendingGate, PlanEntry, SessionEvent, Usage } from "../wire";
import { BACKGROUND_SPEC } from "./background";
import { BLOB_CHART, chartPng, MEDIA_SPEC, SKETCH_ATTACHMENT } from "./media";

/** A session event without its cursor and time (distributes over the kinds). */
export type SessionEventKind = SessionEvent extends infer E ? (E extends unknown ? Omit<E, "cursor" | "t"> : never) : never;
import { earlierScript, itemsAt, type BuiltEarlier } from "./earlier";
import type { Spec } from "./sessions";

/** Fixture-only: scripted transcripts. Cursors follow wire.rs (`seq << 16 | index`). */

const MIN = 60_000;

class Builder {
  events: SessionEvent[] = [];
  seq = 1;
  constructor(public t: number) {}
  push(kind: SessionEventKind, dt = 4_000): SessionEvent {
    this.t += dt;
    const e = { cursor: this.seq++ * 65536, t: this.t, ...kind } as SessionEvent;
    this.events.push(e);
    return e;
  }
  agent(event: AgentEvent, dt?: number) {
    return this.push({ kind: "agent", event }, dt);
  }
  /** A line of a subagent's own conversation. */
  scoped(scope: Scope, event: AgentEvent, dt?: number) {
    return this.agent({ type: "scoped", scope, event }, dt);
  }
  /** The input id of the latest prompt: the turn it starts runs for it. */
  lastInput?: number;
  prompt(text: string, attachments: Attachment[] = []) {
    const c: Command = { command: "prompt", text, attachments };
    this.lastInput = this.seq;
    return this.push({ kind: "input", input: this.seq, command: c, status: { status: "sent" } });
  }
  /** The turn the latest prompt started. */
  turnStarted() {
    return this.agent({ type: "turn_started", trigger: "input", input: this.lastInput });
  }
  tool(name: string, call_id: string, input: JsonValue, ok: boolean, result: JsonValue, dt = 3000, parts: ContentPart[] = []) {
    this.agent({ type: "tool_call_started", name, call_id, input, is_subagent: false }, dt);
    this.agent({ type: "tool_call_finished", call_id, name, ok, result, parts }, 900);
  }
}

/** A session's usage split per model, as an agent that reports one does (the fixture's claude). */
export function modelSplit(driver: string, usage: Usage, cost: number): Record<string, ModelUsage> | undefined {
  return driver === "claude" ? {
    "claude-sonnet-4.5": { input_tokens: Math.round(usage.input_tokens * 0.8), output_tokens: Math.round(usage.output_tokens * 0.85), cost_usd: +(cost * 0.86).toFixed(2), estimated: false },
    "claude-haiku-4.5": { input_tokens: Math.round(usage.input_tokens * 0.2), output_tokens: Math.round(usage.output_tokens * 0.15), cost_usd: +(cost * 0.14).toFixed(2), estimated: false },
  } : undefined;
}

export function buildTranscript(spec: Spec, sessionId: string, now: number): { events: SessionEvent[]; pending: PendingGate[]; plan: PlanEntry[]; earlier?: BuiltEarlier } {
  const start = now - Math.max(spec.agoMin, 1) * MIN - 20 * MIN;
  const b = new Builder(start);
  b.push({ kind: "spawned", generation: 1, pid: 1000 + spec.n, argv: [spec.driver] }, 0);
  b.agent({ type: "session_init", model: undefined, cwd: `~/src/${spec.proj}`, cli_version: "fixture" }, 200);
  // A resumed conversation's earlier history: it is from before this session (yesterday), and its items are not among the events.
  const script = earlierScript(spec, now - 25 * 3_600_000);
  const earlierEvent = script ? b.push({ kind: "earlier", earlier: script.info }, 100) : undefined;
  const plan: PlanEntry[] = [];
  if (spec.n === 7) richAuthTranscript(b, plan);
  else if (spec.n === MEDIA_SPEC.n) mediaTranscript(b, sessionId);
  else if (spec.n === BACKGROUND_SPEC.n) backgroundTranscript(b);
  else if (spec.rounds && spec.rounds >= 100) hugeTranscript(b, spec, spec.rounds);
  else genericTranscript(b, spec, spec.rounds ?? (spec.longHistory ? 70 : 1));

  const nGates = spec.gates?.length ?? 0;
  const lastBefore = b.events[b.events.length - 1].t;
  const gateEvents = (spec.gates ?? []).map((gate) => ({ gate, ev: b.agent({ type: "gate_requested", gate }, 1000) }));
  // Re-time: the newest event lands at the session's last activity; gates are minutes apart.
  const target = now - spec.agoMin * MIN;
  const shift = target - (nGates ? nGates * MIN + 5_000 : 0) - lastBefore;
  b.events.forEach((e) => (e.t += shift));
  const pending: PendingGate[] = gateEvents.map(({ gate, ev }, i) => {
    ev.t = target - (nGates - 1 - i) * MIN - 5_000;
    return { gate, raised_at_ms: ev.t, cursor: ev.cursor };
  });
  void sessionId;
  const earlier = script && earlierEvent ? { info: script.info, items: itemsAt(earlierEvent.cursor, earlierEvent.t, script.items), plan: script.plan } : undefined;
  return { events: b.events, pending, plan, earlier };
}

/**
 * A very long session (thousands of rows) made of many completed turns, each with a user prompt (timestamped),
 * occasional thinking (markdown), an assistant answer (timestamped, markdown), one or two tool rows, and a turn
 * footer ("Done in …"). The last turn is left open, so a `working` session keeps streaming at the bottom. Used by
 * the `?huge=1` fixture to exercise the virtualised transcript.
 */
function hugeTranscript(b: Builder, spec: Spec, turns: number) {
  for (let i = 0; i < turns; i++) {
    b.prompt(`Index and summarise module ${i} of ${spec.proj}.`);
    b.turnStarted();
    if (i % 5 === 0) b.agent({ type: "thinking_delta", text: `Before touching **module ${i}**, I'll re-read its exports and the two call sites, then decide whether the change is safe.\n\n- check \`src/module-${i}.ts\`\n- run the module's tests` }, 1800);
    b.agent({ type: "assistant_delta", text: `Working through module ${i}. It re-exports \`value${i}\` and is imported by two siblings; the change is local, so I'll update it and run \`pnpm test\`.`, message_id: `m${i}` }, 1500);
    b.tool("Read", `r${i}`, { file_path: `src/module-${i}.ts` }, true, `// module ${i}\nexport const value${i} = ${i};\n`);
    if (i % 3 === 2) b.tool("Bash", `b${i}`, { command: `pnpm test src/module-${i}.test.ts` }, i % 11 !== 10, `${30 + (i % 20)} passed${i % 11 === 10 ? " · 1 failed" : ""} · ${(0.4 + (i % 5) * 0.1).toFixed(1)}s`);
    // A completed turn: its footer reads "Done in …" from the start/end gap. Vary the gap so the footers differ.
    b.agent({ type: "turn_completed", was_interrupted: i % 17 === 16, num_turns: 1, usage: { input_tokens: 1800, output_tokens: 320, cache_read_tokens: 900, cache_creation_tokens: 120, cost_usd: 0.01 }, reply: `Module ${i} indexed.` }, 2200 + (i % 7) * 1500);
    if (i === Math.floor(turns / 2)) {
      b.agent({ type: "compaction", info: { trigger: "auto", pre_tokens: 168_400, post_tokens: 41_200, cumulative_dropped_tokens: 127_200, duration_ms: 9_400 } });
      b.agent({ type: "rate_limit_notice", info: { status: "allowed_warning", resets_at: Math.floor(b.t / 1000) + 3 * 3600, rate_limit_type: "five_hour", overage_status: "allowed", is_using_overage: false } });
    }
  }
  // Leave a final turn open so a working session streams new rows at the bottom.
  if (spec.kind === "working") {
    b.prompt(`Now re-index module ${turns} and watch for regressions.`);
    b.turnStarted();
    b.agent({ type: "assistant_delta", text: `On it — re-reading module ${turns} and running the suite.`, message_id: `m${turns}` }, 1500);
    b.agent({ type: "tool_call_started", name: "Read", call_id: "read-live", input: { file_path: `src/module-${turns}.ts` }, is_subagent: false }, 1500);
  }
}

function genericTranscript(b: Builder, spec: Spec, rounds: number) {
  b.prompt(`Work on ${spec.name} in ${spec.proj} (${spec.branch}).`);
  b.turnStarted();
  for (let i = 0; i < rounds; i++) {
    if (i % 7 === 0) b.agent({ type: "thinking_delta", text: "Looking at the surrounding code before changing anything." }, 2000);
    b.agent({ type: "assistant_delta", text: i === 0 ? `Starting on **${spec.name}**. I'll read the relevant files first.` : `Step ${i + 1}: checking the next module and running the tests.`, message_id: `m${i}` }, 2000);
    b.tool("Read", `r${i}`, { file_path: `src/module-${i}.ts` }, true, `// module ${i}\nexport const value${i} = ${i};\n`);
    if (i % 3 === 2) b.tool("Bash", `b${i}`, { command: "pnpm test --silent" }, i % 9 !== 8, `${40 + i} passed${i % 9 === 8 ? " · 1 failed" : ""}`);
  }
  if (spec.kind === "failed") {
    b.tool("Bash", "bfail", { command: "pnpm test" }, false, spec.line);
    b.agent({ type: "assistant_delta", text: `The run failed: ${spec.line}`, message_id: "mf" });
    b.push({ kind: "phase", phase: { state: "failed", reason: spec.line }, cause: { kind: "other" } });
  } else if (spec.kind === "done" || spec.kind === "idle") {
    b.agent({ type: "assistant_delta", text: spec.line, message_id: "mdone" }, 3000);
    const usage = { input_tokens: Math.round(spec.cost * 170_000), output_tokens: Math.round(spec.cost * 15_000), cache_read_tokens: Math.round(spec.cost * 120_000), cache_creation_tokens: Math.round(spec.cost * 9_000), cost_usd: spec.cost };
    const per_model = modelSplit(spec.driver, usage, spec.cost);
    b.agent({ type: "turn_completed", was_interrupted: false, num_turns: 3, usage, per_model, reply: spec.line });
  } else if (spec.kind === "working") {
    b.agent({ type: "tool_call_started", name: "Edit", call_id: "edit-live", input: { file_path: "src-tauri/capabilities/default.json" }, is_subagent: false }, 2000);
  }
  if (spec.longHistory) {
    b.agent({ type: "compaction", info: { trigger: "auto", pre_tokens: 168_400, post_tokens: 41_200, cumulative_dropped_tokens: 127_200, duration_ms: 9_400 } });
    b.agent({ type: "rate_limit_notice", info: { status: "allowed_warning", resets_at: Math.floor(b.t / 1000) + 3 * 3600, rate_limit_type: "five_hour", overage_status: "allowed", is_using_overage: false } });
  }
  if (spec.n === 3) b.agent({ type: "plan_updated", entries: [{ content: "Run vitest on packages/export", priority: "high", status: "in_progress" }] });
}

function richAuthTranscript(b: Builder, plan: PlanEntry[]) {
  b.prompt("Move session auth from express-session to the new `authMiddleware` in packages/auth. Existing cookies must keep working during the rollout. Don't touch the billing routes.");
  b.turnStarted();
  b.agent({ type: "thinking_delta", text: "I need to find every place that reads req.session, then decide how to keep the old cookie valid." }, 14_000);
  b.agent({ type: "assistant_delta", text: "I'll find every place that reads `req.session`, wire the middleware in behind a compat shim, then run the auth test suite.", message_id: "m1" }, 2000);
  b.tool("Grep", "c1", { pattern: "req\\.session", path: "apps/api/src" }, true,
    "apps/api/src/routes/login.ts:22        req.session.userId = user.id\napps/api/src/routes/logout.ts:9        req.session.destroy(done)\napps/api/src/routes/me.ts:14           if (!req.session.userId) return res.sendStatus(401)\napps/api/src/routes/invoices.ts:31     const org = req.session.orgId\n… 10 more");
  b.agent({ type: "tool_call_started", name: "Task", call_id: "c-sub", input: { description: "List routes that depend on session shape", subagent_type: "Explore" }, is_subagent: true, subagent_model: "claude-haiku-4.5" }, 2000);
  b.agent({ type: "task_started", task_id: "task-sub", kind: "local_agent", call_id: "c-sub", description: "List routes that depend on session shape", subagent_type: "Explore", background: false, run_id: "run-sub" }, 100);
  const sub = { call_id: "c-sub", agent_id: "agent-explore-1" };
  b.scoped(sub, { type: "assistant_delta", text: "I'll search the API for every read of `req.session`, then list which fields each route uses.", message_id: "s1" }, 1500);
  b.scoped(sub, { type: "tool_call_started", name: "Grep", call_id: "s-g1", input: { pattern: "req\\.session\\.", path: "apps/api/src" }, is_subagent: false }, 2000);
  b.scoped(sub, { type: "tool_call_finished", name: "Grep", call_id: "s-g1", ok: true, result: "14 matches in 6 files" }, 900);
  b.scoped(sub, { type: "tool_call_started", name: "Read", call_id: "s-r1", input: { file_path: "apps/api/src/routes/invoices.ts" }, is_subagent: false }, 2000);
  b.scoped(sub, { type: "tool_call_finished", name: "Read", call_id: "s-r1", ok: true, result: "// invoices routes\nconst org = req.session.orgId\n…" }, 900);
  b.scoped(sub, { type: "tool_call_started", name: "Read", call_id: "s-r2", input: { file_path: "apps/api/src/routes/members.ts" }, is_subagent: false }, 2000);
  b.scoped(sub, { type: "tool_call_finished", name: "Read", call_id: "s-r2", ok: true, result: "// members routes\nconst org = req.session.orgId\n…" }, 900);
  b.scoped(sub, { type: "assistant_delta", text: "Six routes read `userId`; two of them (invoices, members) also read `orgId`. Billing webhooks never touch the session.", message_id: "s2" }, 2000);
  b.agent({ type: "tool_call_updated", call_id: "c-sub", name: "Task", status: "running", content: [] }, 20_000);
  b.agent({ type: "tool_call_finished", call_id: "c-sub", name: "Task", ok: true,
    result: "- 6 routes read `userId`; 2 also read `orgId` (invoices, members).\n- Billing webhooks never touch the session, safe to leave alone.\n- `connect-redis` TTL is 30 days, so legacy cookies live at most that long." }, 21_000);
  b.agent({ type: "task_ended", task_id: "task-sub", call_id: "c-sub", status: "completed", ok: true, summary: "Listed the routes that depend on the session shape", usage: { total_tokens: 18_400, tool_uses: 3, duration_ms: 41_000 } }, 100);
  b.tool("Edit", "c3", {
    file_path: "apps/api/src/server.ts",
    old_string: "app.use(session({ store: new RedisStore({ client }), secret: env.SESSION_SECRET }))\napp.use(passport.session())",
    new_string: "app.use(legacySessionCompat({ client, until: env.LEGACY_COOKIE_UNTIL }))\napp.use(authMiddleware({ issuer: env.AUTH_ISSUER, cookie: 'll_session' }))",
  }, true, "Edited apps/api/src/server.ts", 3000, [
    { type: "diff", path: "apps/api/src/server.ts", old_text: "app.use(session({ store: new RedisStore({ client }), secret: env.SESSION_SECRET }))\napp.use(passport.session())", new_text: "app.use(legacySessionCompat({ client, until: env.LEGACY_COOKIE_UNTIL }))\napp.use(authMiddleware({ issuer: env.AUTH_ISSUER, cookie: 'll_session' }))" },
  ]);
  b.tool("Bash", "c4", { command: "pnpm --filter api test auth" }, false, "27 passed · 2 failed · 8.4s");
  b.agent({ type: "assistant_delta", text: "The two failures are the cookie-compat tests. Before I fix them I need the session table migration, and a decision on legacy cookies.", message_id: "m2" }, 3000);
  b.agent({ type: "plan_updated", entries: [
    { content: "Map session reads", priority: "high", status: "completed" },
    { content: "Add legacySessionCompat shim", priority: "high", status: "completed" },
    { content: "Wire authMiddleware in server.ts", priority: "high", status: "completed" },
    { content: "Migration for session_tokens", priority: "high", status: "in_progress" },
    { content: "Fix cookie-compat tests", priority: "medium", status: "pending" },
    { content: "Remove express-session dependency", priority: "low", status: "pending" },
  ] });
  plan.push(
    { content: "Map session reads", priority: "high", status: "completed" },
    { content: "Add legacySessionCompat shim", priority: "high", status: "completed" },
    { content: "Wire authMiddleware in server.ts", priority: "high", status: "completed" },
    { content: "Migration for session_tokens", priority: "high", status: "in_progress" },
    { content: "Fix cookie-compat tests", priority: "medium", status: "pending" },
    { content: "Remove express-session dependency", priority: "low", status: "pending" },
  );
}

/** `?media=1`: images in a prompt, a tool result and a reply; files the agent wrote and mentions. */
function mediaTranscript(b: Builder, sessionId: string) {
  b.prompt("Turn data.csv into a bar chart like this sketch, save it as SVG, and write a short PDF report.", [SKETCH_ATTACHMENT(sessionId)]);
  b.turnStarted();
  b.tool("Read", "m1", { file_path: "data.csv" }, true, "quarter,revenue,growth\nQ1,1200,0.05\n…");
  b.tool("Write", "m2", { file_path: "out/chart.svg", content: "<svg …>" }, true, "File created successfully at: out/chart.svg");
  b.tool("Bash", "m3", { command: "rsvg-convert out/chart.svg -o out/chart.png" }, true, "");
  const png = chartPng().length;
  b.tool("Read", "m4", { file_path: "out/chart.png" }, true, [{ type: "image", source: { type: "base64", media_type: "image/png", data: "[omitted]" } }], 3000, [
    { type: "image", mime: "image/png", source: { source: "blob", id: BLOB_CHART, size: png }, width: 240, height: 150 },
  ]);
  b.tool("Write", "m5", { file_path: "report.pdf" }, true, "File created successfully at: report.pdf");
  b.agent({ type: "assistant_delta", text: "Done. The chart is in `out/chart.svg` (a PNG copy in `out/chart.png`), the write-up is `report.pdf`, and `preview.html` shows both. See README.md for notes.", message_id: "mm1" }, 2000);
  b.agent({ type: "assistant_content", parts: [{ type: "file", source: { source: "path", path: "preview.html" }, name: "preview.html", mime: "text/html", uri: "file:///home/you/src/reports/preview.html" }], message_id: "mm1" }, 300);
  b.agent({ type: "assistant_delta", text: "I also added a site and a helper:\n\n```ts\nexport function total(rows: number[]): number {\n  // sum the quarters\n  return rows.reduce((a, b) => a + b, 0);\n}\n```\n\n- `site/index.html` loads `site/style.css` and an image (guide in `docs/guide.md`)\n  - nested item with `inline code`\n- the helper is at `src/app.ts:2`, the notes at `../notes/todo.md` and the folder `../notes/`, your SSH config at `~/.ssh/config`\n\n| quarter | revenue |\n|---|---|\n| Q1 | 1200 |\n\nAnd a very_long_unbroken_token_that_should_wrap_instead_of_overflowing_the_message_column_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx.", message_id: "mm2" }, 2000);
  b.tool("Edit", "m6", { file_path: "src/app.ts", old_string: "return rows.length", new_string: "return rows.reduce((a, b) => a + b, 0)" }, true, "Edited src/app.ts", 3000, [
    { type: "diff", path: "src/app.ts", old_text: "return rows.length", new_text: "return rows.reduce((a, b) => a + b, 0)" },
  ]);
  b.tool("Write", "m7", { file_path: "src/app.ts", content: "export const x: number = 1;\n// done\n" }, true, "File created successfully at: src/app.ts");
  b.tool("Bash", "m8", { command: "git diff --stat && git diff src/app.ts" }, true, "diff --git a/src/app.ts b/src/app.ts\n--- a/src/app.ts\n+++ b/src/app.ts\n@@ -1,3 +1,3 @@\n export const x = 1;\n-const y = 2;\n+const y = 3;\n");
  b.agent({ type: "turn_completed", was_interrupted: false, num_turns: 6, usage: { input_tokens: 2000, output_tokens: 400, cache_read_tokens: 0, cache_creation_tokens: 0, cost_usd: 0.12 } }, 800);
}

/** `?background=1`: subagents and a shell command that outlive the turn, and a turn the agent starts by itself (see `BACKGROUND_SPEC`). */
function backgroundTranscript(b: Builder) {
  b.prompt("Audit every route for a missing auth check. Use subagents for the two big modules and run the whole test suite in the background; I will check back.");
  b.turnStarted();
  b.agent({ type: "assistant_delta", text: "I'll audit the billing and invoice routes with a subagent each, and run the test suite in the background.", message_id: "bg1" }, 2000);
  const sub = (call: string, task: string, run: string, type: string, description: string) => {
    b.agent({ type: "tool_call_started", name: "Agent", call_id: call, input: { description, subagent_type: type }, is_subagent: true, subagent_model: "claude-haiku-4.5" }, 1500);
    b.agent({ type: "task_started", task_id: task, kind: "local_agent", call_id: call, description, subagent_type: type, background: true, run_id: run }, 60);
    // The call returns at once: the subagent keeps running.
    b.agent({ type: "tool_call_finished", call_id: call, name: "Agent", ok: true, result: `Launched ${type} in the background: ${description}` }, 40);
  };
  sub("c-bill", "task-bill", "run-bill", "Explore", "Audit the billing routes");
  sub("c-inv", "task-inv", "run-inv", "Explore", "Audit the invoice routes");
  b.agent({ type: "tool_call_started", name: "Bash", call_id: "c-suite", input: { command: "pnpm test --run", description: "Run the whole test suite" }, is_subagent: false }, 1500);
  b.agent({ type: "task_started", task_id: "task-suite", kind: "local_bash", call_id: "c-suite", description: "pnpm test --run", background: true, run_id: "run-suite" }, 60);
  b.agent({ type: "tool_call_finished", call_id: "c-suite", name: "Bash", ok: true, result: "Command running in background with ID: task-suite" }, 40);
  b.agent({ type: "background_tasks_changed", tasks: [
    { task_id: "task-bill", kind: "local_agent", description: "Audit the billing routes", subagent_type: "Explore", run_id: "run-bill" },
    { task_id: "task-inv", kind: "local_agent", description: "Audit the invoice routes", subagent_type: "Explore", run_id: "run-inv" },
    { task_id: "task-suite", kind: "local_bash", description: "pnpm test --run", run_id: "run-suite" },
  ] }, 20);
  b.agent({ type: "assistant_delta", text: "All three are running. I'll report when they finish.", message_id: "bg2" }, 1500);
  b.agent({ type: "turn_completed", was_interrupted: false, num_turns: 1, usage: { input_tokens: 9_200, output_tokens: 410, cache_read_tokens: 0, cache_creation_tokens: 0, cost_usd: 0.09 }, reply: "All three are running. I'll report when they finish." }, 800);

  // The turn is over and the session is still working: the subagents go on.
  const invoice = { call_id: "c-inv", agent_id: "agent-inv-1" };
  const billing = { call_id: "c-bill", agent_id: "agent-bill-1" };
  const read = (scope: Scope, id: string, file: string) => {
    b.scoped(scope, { type: "tool_call_started", name: "Read", call_id: id, input: { file_path: file }, is_subagent: false }, 2500);
    b.scoped(scope, { type: "tool_call_finished", name: "Read", call_id: id, ok: true, result: `// ${file}\nrouter.get("/", requireAuth, handler)\n…` }, 700);
  };
  b.scoped(invoice, { type: "assistant_delta", text: "I'll read each invoice route and check it passes through `requireAuth`.", message_id: "i1" }, 3000);
  read(invoice, "i-r1", "apps/api/src/routes/invoices.ts");
  read(invoice, "i-r2", "apps/api/src/routes/invoice-pdf.ts");
  b.agent({ type: "task_progress", task_id: "task-inv", call_id: "c-inv", description: "Audit the invoice routes", last_tool: "Read", usage: { total_tokens: 9_800, tool_uses: 2, duration_ms: 14_000 } }, 200);
  b.scoped(invoice, { type: "assistant_delta", text: "Both invoice routes are guarded. `GET /invoices/:id/pdf` also checks the organisation.", message_id: "i2" }, 2500);
  b.agent({ type: "task_ended", task_id: "task-inv", call_id: "c-inv", status: "completed", ok: true, summary: "Both invoice routes are guarded; the PDF route checks the organisation too.", usage: { total_tokens: 11_300, tool_uses: 2, duration_ms: 19_000 } }, 1500);
  b.agent({ type: "background_tasks_changed", tasks: [
    { task_id: "task-bill", kind: "local_agent", description: "Audit the billing routes", subagent_type: "Explore", run_id: "run-bill" },
    { task_id: "task-suite", kind: "local_bash", description: "pnpm test --run", run_id: "run-suite" },
  ] }, 20);

  // The agent turns the subagent's report over to the model: a turn nobody asked for.
  b.agent({ type: "turn_started", trigger: "agent" }, 900);
  b.agent({ type: "assistant_delta", text: "The invoice audit is done: both routes are guarded. Still waiting on the billing audit and the test suite.", message_id: "bg3" }, 1800);
  b.agent({ type: "turn_completed", was_interrupted: false, num_turns: 1, usage: { input_tokens: 4_100, output_tokens: 160, cache_read_tokens: 0, cache_creation_tokens: 0, cost_usd: 0.04 }, reply: "The invoice audit is done: both routes are guarded. Still waiting on the billing audit and the test suite.", origin: { kind: "task-notification", producer: "session-task", run_id: "run-inv" } }, 600);

  b.scoped(billing, { type: "assistant_delta", text: "The billing module has webhook handlers and the customer-facing routes; I'll check both.", message_id: "b1" }, 2500);
  read(billing, "b-r1", "apps/api/src/routes/billing.ts");
  read(billing, "b-r2", "apps/api/src/routes/billing-webhooks.ts");
  read(billing, "b-r3", "apps/api/src/routes/plans.ts");
  b.agent({ type: "task_progress", task_id: "task-bill", call_id: "c-bill", description: "Audit the billing routes", last_tool: "Read", usage: { total_tokens: 15_600, tool_uses: 3, duration_ms: 31_000 } }, 200);
  b.agent({ type: "task_progress", task_id: "task-suite", description: "pnpm test --run", usage: { total_tokens: 0, tool_uses: 0, duration_ms: 34_000 } }, 200);
}
