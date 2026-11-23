import type { AgentEvent, Attachment, Command, ContentPart, JsonValue, PendingGate, PlanEntry, SessionEvent } from "../wire";
import { BLOB_CHART, chartPng, MEDIA_SPEC, SKETCH_ATTACHMENT } from "./media";

/** A session event without its cursor and time (distributes over the kinds). */
export type SessionEventKind = SessionEvent extends infer E ? (E extends unknown ? Omit<E, "cursor" | "t"> : never) : never;
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
  prompt(text: string, attachments: Attachment[] = []) {
    const c: Command = { command: "prompt", text, attachments };
    return this.push({ kind: "input", input: this.seq, command: c, status: { status: "sent" } });
  }
  tool(name: string, call_id: string, input: JsonValue, ok: boolean, result: JsonValue, dt = 3000, parts: ContentPart[] = []) {
    this.agent({ type: "tool_call_started", name, call_id, input, is_subagent: false }, dt);
    this.agent({ type: "tool_call_finished", call_id, name, ok, result, parts }, 900);
  }
}

export function buildTranscript(spec: Spec, sessionId: string, now: number): { events: SessionEvent[]; pending: PendingGate[]; plan: PlanEntry[] } {
  const start = now - Math.max(spec.agoMin, 1) * MIN - 20 * MIN;
  const b = new Builder(start);
  b.push({ kind: "spawned", generation: 1, pid: 1000 + spec.n, argv: [spec.driver] }, 0);
  b.agent({ type: "session_init", model: undefined, cwd: `~/src/${spec.proj}`, cli_version: "fixture" }, 200);
  const plan: PlanEntry[] = [];
  if (spec.n === 7) richAuthTranscript(b, plan);
  else if (spec.n === MEDIA_SPEC.n) mediaTranscript(b, sessionId);
  else genericTranscript(b, spec, spec.longHistory ? 70 : 1);

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
  return { events: b.events, pending, plan };
}

function genericTranscript(b: Builder, spec: Spec, rounds: number) {
  b.prompt(`Work on ${spec.name} in ${spec.proj} (${spec.branch}).`);
  b.agent({ type: "turn_started" });
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
    const per_model = spec.driver === "claude" ? {
      "claude-sonnet-4.5": { input_tokens: Math.round(usage.input_tokens * 0.8), output_tokens: Math.round(usage.output_tokens * 0.85), cost_usd: +(spec.cost * 0.86).toFixed(2), estimated: false },
      "claude-haiku-4.5": { input_tokens: Math.round(usage.input_tokens * 0.2), output_tokens: Math.round(usage.output_tokens * 0.15), cost_usd: +(spec.cost * 0.14).toFixed(2), estimated: false },
    } : undefined;
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
  b.agent({ type: "turn_started" });
  b.agent({ type: "thinking_delta", text: "I need to find every place that reads req.session, then decide how to keep the old cookie valid." }, 14_000);
  b.agent({ type: "assistant_delta", text: "I'll find every place that reads `req.session`, wire the middleware in behind a compat shim, then run the auth test suite.", message_id: "m1" }, 2000);
  b.tool("Grep", "c1", { pattern: "req\\.session", path: "apps/api/src" }, true,
    "apps/api/src/routes/login.ts:22        req.session.userId = user.id\napps/api/src/routes/logout.ts:9        req.session.destroy(done)\napps/api/src/routes/me.ts:14           if (!req.session.userId) return res.sendStatus(401)\napps/api/src/routes/invoices.ts:31     const org = req.session.orgId\n… 10 more");
  b.agent({ type: "tool_call_started", name: "Task", call_id: "c-sub", input: { description: "List routes that depend on session shape", subagent_type: "Explore" }, is_subagent: true, subagent_model: "claude-haiku-4.5" }, 2000);
  b.agent({ type: "tool_call_updated", call_id: "c-sub", name: "Task", status: "running", content: [] }, 20_000);
  b.agent({ type: "tool_call_finished", call_id: "c-sub", name: "Task", ok: true,
    result: "- 6 routes read `userId`; 2 also read `orgId` (invoices, members).\n- Billing webhooks never touch the session, safe to leave alone.\n- `connect-redis` TTL is 30 days, so legacy cookies live at most that long." }, 21_000);
  b.tool("Edit", "c3", {
    file_path: "apps/api/src/server.ts",
    old_string: "app.use(session({ store: new RedisStore({ client }), secret: env.SESSION_SECRET }))\napp.use(passport.session())",
    new_string: "app.use(legacySessionCompat({ client, until: env.LEGACY_COOKIE_UNTIL }))\napp.use(authMiddleware({ issuer: env.AUTH_ISSUER, cookie: 'll_session' }))",
  }, true, "Edited apps/api/src/server.ts");
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
  b.agent({ type: "turn_started" });
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
  b.tool("Edit", "m6", { file_path: "src/app.ts", old_string: "return rows.length", new_string: "return rows.reduce((a, b) => a + b, 0)" }, true, "Edited src/app.ts");
  b.tool("Write", "m7", { file_path: "src/app.ts", content: "export const x: number = 1;\n// done\n" }, true, "File created successfully at: src/app.ts");
  b.tool("Bash", "m8", { command: "git diff --stat && git diff src/app.ts" }, true, "diff --git a/src/app.ts b/src/app.ts\n--- a/src/app.ts\n+++ b/src/app.ts\n@@ -1,3 +1,3 @@\n export const x = 1;\n-const y = 2;\n+const y = 3;\n");
  b.agent({ type: "turn_completed", was_interrupted: false, num_turns: 6, usage: { input_tokens: 2000, output_tokens: 400, cache_read_tokens: 0, cache_creation_tokens: 0, cost_usd: 0.12 } }, 800);
}
