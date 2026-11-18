import type { AgentEvent, Command, PendingGate, PlanEntry, SessionEvent, SessionEventKind } from "../types";
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
  prompt(text: string) {
    const c: Command = { command: "prompt", text };
    return this.push({ kind: "input", command: c, status: { status: "sent" } });
  }
  tool(name: string, call_id: string, input: unknown, ok: boolean, result: unknown, dt = 3000) {
    this.agent({ type: "tool_call_started", name, call_id, input, is_subagent: false }, dt);
    this.agent({ type: "tool_call_finished", call_id, name, ok, result }, 900);
  }
}

export function buildTranscript(spec: Spec, sessionId: string, now: number): { events: SessionEvent[]; pending: PendingGate[]; plan: PlanEntry[] } {
  const start = now - Math.max(spec.agoMin, 1) * MIN - 20 * MIN;
  const b = new Builder(start);
  b.push({ kind: "spawned", generation: 1, pid: 1000 + spec.n, argv: [spec.driver] }, 0);
  b.agent({ type: "session_init", model: undefined, cwd: `~/src/${spec.proj}`, cli_version: "fixture" }, 200);
  const plan: PlanEntry[] = [];
  if (spec.n === 7) richAuthTranscript(b, plan);
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
    b.push({ kind: "phase", phase: { state: "failed", reason: spec.line } });
  } else if (spec.kind === "done" || spec.kind === "idle") {
    b.agent({ type: "assistant_delta", text: spec.line, message_id: "mdone" }, 3000);
    b.agent({ type: "turn_completed", was_interrupted: false, num_turns: 3, usage: { input_tokens: 1, output_tokens: 1, cache_read_tokens: 0, cache_creation_tokens: 0, cost_usd: spec.cost }, reply: spec.line });
  } else if (spec.kind === "working") {
    b.agent({ type: "tool_call_started", name: "Edit", call_id: "edit-live", input: { file_path: "src-tauri/capabilities/default.json" }, is_subagent: false }, 2000);
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
