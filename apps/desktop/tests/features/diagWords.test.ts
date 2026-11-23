import { describe, expect, it } from "vitest";
import type { TrailEntry, TrailItem, UiEvent } from "../../src/bridge/wire";
import { parseRecord, type DiagRecord } from "../../src/lib/diagRecord";
import { WORDED_EVENTS, appEventWords, causeWords, clockMs, duration, errorWords, recordFields, recordTone, recordWords, sizeWords, trailFields, trailTone, trailWords } from "../../src/features/diagnostics/words";

const TRACE = "u-0123456789abcdef";
const ERR = { code: "refused", len: 12, fp: "abcd1234" };
const ERR_WORDS = "refused (12 characters long, fingerprint abcd1234; the text itself is never recorded)";

/** One sample of every `TrailEntry` variant: adding a variant to the wire types fails the build until it is worded and listed here. */
const SAMPLES: { [K in TrailEntry["ev"]]: [Extract<TrailEntry, { ev: K }>, string] } = {
  "process.spawned": [{ ev: "process.spawned", generation: 1, argc: 6, flags: ["--resume", "--model"], pid: 4242 }, "A process was started (generation 1, pid 4242), 6 arguments, flags --resume --model"],
  "process.started": [{ ev: "process.started", generation: 2, resume: "resume", resume_id: "n-1", model: "m1", mode: "plan", first_input: 7, trace: TRACE }, "Start (generation 2): resuming a conversation n-1, model m1, mode plan, with a first prompt (input #7)"],
  "agent.init": [{ ev: "agent.init", tools: 14, mcp_servers: 1, slash_commands: 9, native_session_id: "n-1", model: "m1", cli_version: "2.1.4", mode: "default" }, "The agent introduced itself — its session n-1, version 2.1.4, model m1, mode default; 14 tools, 1 MCP server, 9 slash commands"],
  "input.applied": [{ ev: "input.applied", input: 3, kind: "prompt", chars: 61, attachments: 2, bytes: 2048, status: "sent", trace: TRACE }, "A prompt was sent (61 characters, 2 attachments (2.0 KB)) — #3, handed to the agent"],
  "input.sent": [{ ev: "input.sent", input: 5 }, "Input #5 left the queue and is being written to the agent"],
  "input.written": [{ ev: "input.written", input: 5 }, "Input #5 was written to the agent's input"],
  "input.write_failed": [{ ev: "input.write_failed", input: 5, error: { code: "stdin_write", text: "write to child stdin: broken pipe" } }, "Writing to the agent failed for input #5 — stdin_write: write to child stdin: broken pipe"],
  "input.acknowledged": [{ ev: "input.acknowledged", input: 5, steered: true }, "The agent acknowledged input #5 (steered into the running turn)"],
  "input.dropped": [{ ev: "input.dropped", input: 5, reason: "interrupted" }, "Input #5 will never reach the agent — it was interrupted while it waited"],
  "turn.started": [{ ev: "turn.started", trigger: "input", input: 5, native_turn_id: "t-9" }, "A turn started for input #5 (turn t-9)"],
  "turn.completed": [{ ev: "turn.completed", interrupted: false, num_turns: 3, input_tokens: 1200, output_tokens: 340, cost_usd_micros: 41_000, reply_chars: 900, end_reason: "max_turns" }, "The turn ended (the turn limit was reached): 3 turns, 1200 tokens in, 340 out, $0.0410, a reply of 900 characters"],
  "tool.started": [{ ev: "tool.started", call_id: "c1", name: "Read", subagent: false }, "Tool Read started — call c1"],
  "tool.finished": [{ ev: "tool.finished", call_id: "c1", ok: false }, "Tool call c1 failed"],
  "task.started": [{ ev: "task.started", task_id: "t1", background: true, kind: "local_agent", call_id: "c9" }, "A background task started — t1 (local_agent), spawned by call c9"],
  "task.progress": [{ ev: "task.progress", task_id: "t1", tool: "Read", total_tokens: 900, tool_uses: 3, duration_ms: 14_000 }, "Task t1 reported progress: last tool Read, 3 tool uses, 900 tokens in 14 s"],
  "task.updated": [{ ev: "task.updated", task_id: "t1", status: "completed" }, "Task t1 reported the status completed"],
  "task.ended": [{ ev: "task.ended", task_id: "t1", ok: true, summary_chars: 62, status: "completed", call_id: "c9", total_tokens: 1100, duration_ms: 19_000 }, "Task t1 ended successfully (completed), spawned by call c9, 1100 tokens in 19 s, a summary of 62 characters"],
  "background.tasks": [{ ev: "background.tasks", running: 2 }, "The agent lists 2 background tasks as running"],
  "gate.raised": [{ ev: "gate.raised", request_id: "p-1", gate_type: "permission", choices: 3, tool: "Bash" }, "Waiting for you — the agent asked for a permission decision on Bash (3 choices) — request p-1"],
  "gate.resolved": [{ ev: "gate.resolved", request_id: "p-1", reason: "withdrawn" }, "Request p-1 is no longer pending (withdrawn)"],
  "agent.model": [{ ev: "agent.model", model: "m2" }, "The model is now m2"],
  "agent.mode": [{ ev: "agent.mode", mode: "plan" }, "The mode is now plan"],
  "agent.rejected": [{ ev: "agent.rejected", command: "set_model", error: ERR }, `The agent rejected a command (set_model) — ${ERR_WORDS}`],
  "agent.notice": [{ ev: "agent.notice", error: ERR }, `The agent sent a notice — ${ERR_WORDS}`],
  "agent.compaction": [{ ev: "agent.compaction", pre_tokens: 168_400, post_tokens: 41_200, duration_ms: 9_400, trigger: "auto" }, "The conversation was compacted: 168400 → 41200 tokens in 9.4 s (auto)"],
  "agent.rate_limited": [{ ev: "agent.rate_limited", resets_at: 1_700_000_000, is_using_overage: false, status: "allowed_warning", rate_limit_type: "five_hour" }, "The provider reports a rate limit: allowed_warning (five_hour), resets at 22:13:20 UTC"],
  "agent.unknown": [{ ev: "agent.unknown", raw_type: "ping" }, "The agent sent something Marsad does not recognise (ping)"],
  "process.stderr": [{ ev: "process.stderr", chars: 118 }, "The agent wrote 118 characters to its error output"],
  phase: [{ ev: "phase", to: "idle", cause: { kind: "turn_completed", interrupted: false, end_reason: null } }, "Idle — the turn ended"],
  "process.exited": [{ ev: "process.exited", code: 1, signal: null }, "The process exited with code 1"],
  "process.lost": [{ ev: "process.lost", reason: "the shim vanished" }, "The host lost the process — the shim vanished"],
  "runtime.notice": [{ ev: "runtime.notice", code: "stop_requested" }, "A graceful stop was requested"],
  anomaly: [{ ev: "anomaly", kind: "activity_while_idle", first: "tool_call_started" }, "The agent produced output after its turn had ended (first: tool_call_started)"],
  "earlier.history": [{ ev: "earlier.history", state: "loaded", items: 140, omitted: 0, shortened: 0, unreadable: 0, unrecognised: 0, complete: true, interrupted: false, source: "agent_storage" }, "Earlier history loaded: 140 items (complete), read from the agent's own storage"],
};

describe("trail words", () => {
  it.each(Object.entries(SAMPLES))("%s", (_ev, [entry, words]) => {
    expect(trailWords(entry as TrailEntry)).toBe(words);
  });

  it("every phase cause has its words (adding a cause to the wire types fails the build until it is listed here)", () => {
    type Cause = Parameters<typeof causeWords>[0];
    const causes: { [K in Cause["kind"]]: [Extract<Cause, { kind: K }>, string][] } = {
      spawned: [[{ kind: "spawned", generation: 2 }, "the process (generation 2) was started"]],
      handshake_done: [[{ kind: "handshake_done" }, "the agent finished starting up"]],
      turn_started: [
        [{ kind: "turn_started", trigger: "input", input: 4 }, "a turn started for input #4"],
        [{ kind: "turn_started", trigger: "input" }, "a turn started"],
        [{ kind: "turn_started", trigger: "agent" }, "the agent started a turn on its own"],
        [{ kind: "turn_started", trigger: "unreported" }, "a turn started (the agent did not say what started it)"],
      ],
      task_started: [[{ kind: "task_started", task_id: "t1" }, "a background task started (t1)"]],
      task_ended: [[{ kind: "task_ended", task_id: "t1" }, "background task t1 ended"]],
      turn_completed: [
        [{ kind: "turn_completed", interrupted: true }, "the turn was interrupted"],
        [{ kind: "turn_completed", interrupted: false, end_reason: "execution_error" }, "the turn ended (the agent reported an error)"],
      ],
      gate_raised: [
        [{ kind: "gate_raised", request_id: "p-1", external: false }, "a request is waiting for your decision (p-1)"],
        [{ kind: "gate_raised", request_id: "w-1", external: true }, "a request from outside the agent is waiting for your decision (w-1)"],
      ],
      gate_cleared: [[{ kind: "gate_cleared", request_id: "p-1", how: "answered" }, "request p-1 is no longer pending (you answered it)"]],
      exited: [
        [{ kind: "exited", requested: true, handshake_timeout: false, code: 0, signal: null }, "the process exited with code 0 after a stop was requested"],
        [{ kind: "exited", requested: false, handshake_timeout: true, code: null, signal: 9 }, "the process exited on signal 9 (the startup deadline had passed)"],
      ],
      lost: [[{ kind: "lost" }, "the host lost track of the process"]],
      other: [[{ kind: "other" }, "no more specific cause was recorded"]],
    };
    for (const samples of Object.values(causes)) for (const [c, w] of samples as [Cause, string][]) expect(causeWords(c)).toBe(w);
  });

  it("the phase sentence says what the owner reads: the state and why", () => {
    expect(trailWords({ ev: "phase", to: "needs_you", cause: { kind: "gate_raised", request_id: "p-1", external: false } })).toBe("Needs you — a request is waiting for your decision (p-1)");
    expect(trailWords({ ev: "phase", to: "failed", cause: { kind: "exited", requested: false, handshake_timeout: false, code: 1 }, reason: "pnpm test exited 1" })).toBe("Failed — the process exited with code 1: pnpm test exited 1");
  });

  it("every way an input can be dropped, and every status a command can have, is worded", () => {
    const dropped: [string, string][] = [
      ["interrupted", "it was interrupted while it waited"],
      ["shutdown", "the session was shut down first"],
      ["unencodable", "it could not be encoded for the agent"],
      ["refused", "the agent refused it"],
      ["process_ended", "the process ended before the agent took it"],
    ];
    for (const [reason, why] of dropped) expect(trailWords({ ev: "input.dropped", input: 1, reason })).toBe(`Input #1 will never reach the agent — ${why}`);
    expect(trailWords({ ev: "input.dropped", input: 1, reason: "refused", error: ERR })).toBe(`Input #1 will never reach the agent — the agent refused it (${ERR_WORDS})`);
    expect(trailWords({ ev: "input.dropped", input: 1, reason: "from_the_future" })).toBe("Input #1 will never reach the agent — from_the_future");
    const applied = (kind: string, status: string, extra: Partial<Extract<TrailEntry, { ev: "input.applied" }>> = {}) => trailWords({ ev: "input.applied", input: 9, kind, chars: 3, attachments: 0, bytes: 0, status, ...extra });
    const statuses: [string, string][] = [
      ["sent", "handed to the agent"],
      ["queued", "held until the agent can take it"],
      ["unsupported", "this agent cannot do that"],
      ["rejected", "rejected"],
      ["not_running", "the session was not running"],
      ["withdrawn", "the request it answers is gone"],
    ];
    for (const [status, words] of statuses) expect(applied("prompt", status)).toBe(`A prompt was sent (3 characters) — #9, ${words}`);
    expect(applied("prompt", "queued")).toBe("A prompt was sent (3 characters) — #9, held until the agent can take it");
    expect(applied("run_slash", "sent", { target: "review" })).toBe("Slash command /review was run (3 characters of arguments) — #9, handed to the agent");
    expect(applied("answer_gate", "sent", { target: "p-1", choice: "allow", chars: 0 })).toBe("A request (p-1) was answered with allow — #9, handed to the agent");
    expect(applied("answer_question", "sent", { target: "q-1" })).toBe("A question (q-1) was answered (3 characters) — #9, handed to the agent");
    expect(applied("interrupt", "sent")).toBe("An interrupt was sent — #9, handed to the agent");
    expect(applied("set_model", "sent", { target: "m2" })).toBe("The model was set to m2 — #9, handed to the agent");
    expect(applied("set_mode", "sent", { target: "plan" })).toBe("The mode was set to plan — #9, handed to the agent");
    expect(applied("shutdown", "sent")).toBe("A shutdown was sent — #9, handed to the agent");
    expect(applied("prompt", "not_running", { reason: { code: "not_running", len: 30, fp: "0a0b0c0d" } })).toContain("the session was not running (not_running (30 characters long");
    expect(applied("teleport", "sent")).toBe("A command (teleport) was applied — #9, handed to the agent");
  });

  it("every runtime notice code is worded", () => {
    for (const code of ["stop_requested", "handshake_deadline", "signal_sent", "previous_generation_unclosed", "start_ignored", "shim", "unrecognised_note", "other"]) {
      expect(trailWords({ ev: "runtime.notice", code })).not.toContain(code);
    }
    expect(trailWords({ ev: "runtime.notice", code: "from_a_newer_runtime" })).toBe("The runtime noted something (from_a_newer_runtime)");
  });

  it("an error is a code and a length and a fingerprint, or the daemon's own scrubbed words; never anything else", () => {
    expect(errorWords({ code: "io", os: 13 })).toBe("io (OS error 13; the text itself is never recorded)");
    expect(errorWords({ code: "dropped" })).toBe("dropped");
    expect(errorWords({ code: "x", text: "scrubbed words", len: 99 })).toBe("x: scrubbed words");
    expect(errorWords(null)).toBe("");
  });

  it("an entry this build does not know is shown by name and fields, and never throws", () => {
    const future = { ev: "process.teleported", cursor: 5, t: 1, to: "mars", after_ms: 12 } as unknown as TrailEntry;
    expect(trailWords(future)).toBe("process.teleported — to mars, after_ms 12");
    expect(trailTone(future)).toBe("plain");
    expect(trailWords({} as unknown as TrailEntry)).toBe("an unknown trail entry");
    expect(causeWords({ kind: "meteor", size: 3 } as never)).toBe("meteor — size 3");
  });

  it("tones: failures are bad, what needs you is a warning, the rest is plain", () => {
    expect(trailTone(SAMPLES["input.dropped"][0])).toBe("bad");
    expect(trailTone(SAMPLES["input.write_failed"][0])).toBe("bad");
    expect(trailTone(SAMPLES.anomaly[0])).toBe("warn");
    expect(trailTone(SAMPLES["task.ended"][0])).toBe("plain");
    expect(trailTone({ ...SAMPLES["task.ended"][0], ok: false })).toBe("warn");
    expect(trailTone(SAMPLES["gate.raised"][0])).toBe("warn");
    expect(trailTone(SAMPLES["input.acknowledged"][0])).toBe("ok");
    expect(trailTone(SAMPLES["process.exited"][0])).toBe("bad");
    expect(trailTone({ ev: "process.exited" })).toBe("plain");
    expect(trailTone({ ev: "phase", to: "failed", cause: { kind: "other" } })).toBe("bad");
    expect(trailTone({ ev: "phase", to: "working", cause: { kind: "other" } })).toBe("plain");
    expect(trailTone(SAMPLES["tool.started"][0])).toBe("plain");
  });

  it("the expanded view lists every field the host sent, with the time", () => {
    const item = { cursor: 65536, t: 1_700_000_000_123, ...SAMPLES["input.applied"][0] } as TrailItem;
    const f = Object.fromEntries(trailFields(item).map((x) => [x.k, x.v]));
    expect(f).toMatchObject({ cursor: "65536", ev: "input.applied", input: "3", kind: "prompt", status: "sent", trace: TRACE });
    expect(f.time).toMatch(/^\d\d:\d\d:\d\d\.123 \(1700000000123\)$/);
  });
});

const rec = (ev: string, fields: Record<string, unknown> = {}, lvl = "info"): DiagRecord => parseRecord(JSON.stringify({ b: "b1", seq: 1, t: 1_700_000_000_000, m: 5, src: "hub", lvl, ev, ...fields }))!;

/** One sample of every event name that is worded specially. */
const RECORDS: Record<string, [Record<string, unknown>, string]> = {
  "link.attempt_begin": [{ link: "hosts:gpu", n: 7 }, "Connecting to gpu — attempt 7 began"],
  "link.attempt_end": [{ link: "hosts:gpu", n: 7, outcome: "backoff", error: { code: "unreachable", text: "could not reach the host" }, ms: 10_034 }, "Attempt 7 failed; the next attempt follows the backoff schedule after 10 s — unreachable: could not reach the host"],
  "link.state": [{ link: "hosts:gpu", from: "connecting", to: "reconnecting", attempt: 3, error: "connection reset by peer" }, "gpu is reconnecting (was connecting), attempt 3 — connection reset by peer"],
  "link.provision": [{ link: "hosts:pi", reason: "build_mismatch", outcome: "failed_retryable", ms: 3000, error: { code: "provision_failed", len: 40, fp: "ffff0000" } }, "Helper on pi (build_mismatch): failed_retryable after 3.0 s — provision_failed (40 characters long, fingerprint ffff0000; the text itself is never recorded)"],
  "link.closed": [{ link: "hosts:win", reason: "io", connected_ms: 7_200_000, transport_exit: 255, stderr: "Connection reset by peer" }, "The connection to win closed (an I/O error) after being up 120 min 0 s, the transport exited with code 255 — Connection reset by peer"],
  "transport.spawned": [{ link: "hosts:hz", n: 1, program: "ssh", argc: 9, pid: 20001 }, "Started ssh for hz (attempt 1; 9 arguments, pid 20001)"],
  "transport.exited": [{ link: "hosts:hz", n: 1, code: 255, stderr_bytes: 62 }, "The transport to hz exited (attempt 1) with code 255; 62 B of error output"],
  "conn.accepted": [{ role: "app", peer_build: "0.7.3", peer_host: "x1" }, "A peer connected (role app, build 0.7.3, host x1)"],
  "conn.handshake_failed": [{ error: { code: "protocol", len: 5, fp: "00000000" } }, "A connection failed its handshake — protocol (5 characters long, fingerprint 00000000; the text itself is never recorded)"],
  "conn.closed": [{ role: "hub", reason: "eof", ms: 5000 }, "A peer connection closed (role hub, eof) after 5.0 s"],
  "rpc.traced": [{ trace: TRACE, method: "session.command", host: "hz", hops: 1, ms: 12, outcome: "error", code: "timeout" }, "session.command for hz (1 hop) error in 12 ms (timeout)"],
  "rpc.forward_failed": [{ method: "session.command", host: "win", code: "upstream", ms: 4 }, "Could not forward session.command to win — upstream after 4 ms"],
  "sub.closed": [{ topic: "session.events", host: "hz", reason: "lagged" }, "A session.events subscription on hz ended: lagged"],
  "boot.attempt": [{ host: "gpu", n: 1, shell: "posix", first_contact: true, interactive: true, armed: false }, "Bootstrap attempt 1: posix shell, first contact, sign-in can be asked"],
  "boot.preamble": [{ host: "gpu", n: 1, lines: 4, bytes: 210, shape: "Welcome to Ubuntu" }, "The login shell printed 4 lines (210 B) before the bootstrap — Welcome to Ubuntu"],
  "boot.report": [{ host: "hz", n: 1, os: "linux", arch: "x86_64", installed: "0.7.3", ms: 372 }, "The host reports linux/x86_64, helper 0.7.3 (372 ms)"],
  "boot.windows_retry": [{ host: "win", n: 2, why: "mingw_sh" }, "Trying again for Windows: a POSIX shell answered on Windows"],
  "boot.action": [{ host: "pi", n: 1, action: "upload", bytes: 4_300_000, target: "0.7.3" }, "Helper: upload (4.1 MB) → 0.7.3"],
  "boot.uploaded": [{ host: "pi", n: 1, bytes: 4_300_000, ms: 2_000 }, "Uploaded the helper (4.1 MB) in 2.0 s"],
  "boot.ok": [{ host: "hz", n: 1, ms: 38 }, "The helper started on the host in 38 ms"],
  "boot.hello": [{ host: "hz", n: 1, build: "0.7.3", host_id_matches: false, machine: "m-1", ms: 44 }, "The helper answered (build 0.7.3) in 44 ms, but under another host id, machine m-1"],
  "boot.failed": [{ host: "gpu", n: 1, stage: "hello", kind: "unreachable", exit: 255, stderr: "ssh: connect to host gpu port 22: Connection timed out", error: { code: "unreachable", text: "could not reach the host" } }, "Bootstrap failed at hello (unreachable), exit code 255 — ssh: connect to host gpu port 22: Connection timed out — unreachable: could not reach the host"],
  "boot.twin": [{ host: "a", n: 1, twin: "b" }, "This host reaches the same machine as b"],
  "boot.id_mismatch": [{ host: "a", n: 1, theirs: "z" }, "The machine answered as host z, not as this one"],
  "boot.build_mismatch": [{ host: "pi", expected: "0.7.3", found: "0.7.1" }, "The helper is build 0.7.1; 0.7.3 is expected"],
  "auth.prompt": [{ host: "gpu", prompt: 4, kind: "secret", attempt: 2 }, "Sign-in prompt 4 (secret), attempt 2"],
  "auth.answered": [{ host: "gpu", prompt: 4, ms: 5300 }, "Sign-in prompt 4 was answered after 5.3 s"],
  "auth.cancelled": [{ host: "gpu", prompt: 4, why: "timeout" }, "Sign-in prompt 4 was cancelled: it timed out"],
  "hosts.changed": [{ host: "gpu", op: "added", kind: "ssh", target_kind: "user_at_host" }, "Host added (ssh, target user_at_host)"],
  "hosts.learned": [{ host: "gpu", what: "platform" }, "Learned the host's platform"],
  "hosts.held": [{ host: "gpu" }, "gpu needs you at sign-in, so the hub did not connect it: it waits for you to ask (no automatic retry)"],
  "hosts.failed": [{ host: "gpu", op: "learn", error: { code: "io", os: 5 } }, "The host registry failed at learn — io (OS error 5; the text itself is never recorded)"],
  "hostenv.apply": [{ host: "hz", revision: "r3", outcome: "failed", ms: 120, setup_ok: false, error: { code: "setup", text: "exit 2" } }, "Host settings failed (revision r3) in 120 ms, the setup commands failed — setup: exit 2"],
  "fleet.feed": [{ host: "hz", event: "lagged" }, "The session feed of hz lagged"],
  launch: [{ trace: TRACE, host: "hz", session: "s-1", driver: "claude", resume: "fresh", first_prompt: true, ms: 800, outcome: "ok" }, "Launch of claude on hz (fresh) ok in 800 ms — session s-1, with a first prompt"],
  "daemon.start": [{ hub: true, host_id: "x1", build: "0.7.3", pid: 42 }, "Daemon started (hub x1, build 0.7.3, pid 42)"],
  "daemon.ready": [{ total_ms: 212, env_ms: 120, adopt_ms: 31, node_ms: 9 }, "Daemon ready in 212 ms (environment 120 ms, adopting sessions 31 ms, node 9 ms)"],
  "daemon.readiness_unwritten": [{ why: "pipe_closed", error: { code: "io" } }, "The daemon could not tell its starter it was ready (pipe_closed) — io"],
  "daemon.lock_held": [{ lock: "~/.marsad/lock" }, "Another daemon holds this machine's lock"],
  "daemon.failed": [{ stage: "listen", exit: 70 }, "The daemon failed at listen (exit code 70)"],
  "daemon.shutdown": [{ why: "signal", signal: "SIGTERM" }, "Daemon shut down: a signal (SIGTERM)"],
  "bridge.start": [{ host_id: "hz", pid: 9 }, "The bridge started for hz (pid 9)"],
  "bridge.daemon": [{ outcome: "start_failed", ms: 900, reason: "exited_before_ready", exit: 1 }, "The bridge could not start the daemon (900 ms): exited_before_ready, exit code 1"],
  "bridge.end": [{ reason: "io_error" }, "The bridge ended (io_error)"],
  "stop.end": [{ outcome: "not_exited", ms: 3000 }, "Stopping the daemon: not_exited in 3.0 s"],
  "askpass.end": [{ outcome: "refused", ms: 12 }, "The sign-in helper ended: refused in 12 ms"],
  "session.command_refused": [{ session: "s-1", trace: TRACE, kind: "prompt", code: "busy" }, "The prompt for session s-1 was refused before it was recorded (busy)"],
  "session.command_timeout": [{ session: "s-1", kind: "interrupt" }, "The interrupt for session s-1 timed out waiting; it may still have been applied"],
  "env.capture": [{ source: "login_shell", ms: 142 }, "Captured the login environment (login_shell) in 142 ms"],
  "env.setup": [{ ok: false, ms: 9000 }, "The host's setup commands failed in 9.0 s"],
  "session.lost": [{ session: "s-1", error: { code: "shim_unreachable", len: 30, fp: "aaaa0000" } }, "Session s-1 lost its process — shim_unreachable (30 characters long, fingerprint aaaa0000; the text itself is never recorded)"],
  "session.retry": [{ session: "s-1", attempt: 2, error: { code: "shim_unreachable" } }, "Reconnecting to the process of session s-1 — attempt 2 — shim_unreachable"],
  "session.op_failed": [{ session: "s-1", op: "signal", error: { code: "shim_failed" } }, "signal on session s-1 failed — shim_failed"],
  "session.outbox_overflow": [{ session: "s-1" }, "Session s-1 fell too far behind its process"],
  "runtime.adopt_failed": [{ session: "s-1", error: { code: "adopt" } }, "Session s-1 could not be taken over at start — adopt"],
  "process.start": [{ role: "hub", build: "0.7.3", pid: 4 }, "Process started (hub, build 0.7.3, pid 4)"],
  "process.exit": [{ reason: "signal" }, "Process exiting (signal)"],
  "diag.dropped": [{ count: 12, first_seq: 40, last_seq: 51 }, "12 diagnostics records could not be written (sequence 40–51)"],
  "diag.suppressed": [{ event: "link.state", count: 300 }, "300 records of link.state left out: too many in a minute"],
  "diag.write_failed": [{ error: { code: "io", os: 28 } }, "The log file could not be written — io (OS error 28; the text itself is never recorded)"],
  "diag.panic": [{ location: "src/x.rs:1:2", thread: "main" }, "A panic at src/x.rs:1:2 on thread main"],
  "trace.counted": [{ target: "hyper", level: "warn", n: 5 }, "5 messages at warn from hyper (counted, not recorded)"],
};

describe("log record words", () => {
  it("every event worded specially has a sample here, and the sample reads as written", () => {
    expect([...WORDED_EVENTS].sort()).toEqual(Object.keys(RECORDS).sort());
    for (const [ev, [fields, words]] of Object.entries(RECORDS)) expect(recordWords(rec(ev, fields)), ev).toBe(words);
  });

  it("a record missing every field (a daemon of another version) is still worded and never throws", () => {
    for (const ev of WORDED_EVENTS) {
      const w = recordWords(rec(ev));
      expect(w.length, ev).toBeGreaterThan(3);
      expect(w, ev).not.toMatch(/undefined|NaN|\[object/);
    }
  });

  it("fields of the wrong type never throw either", () => {
    for (const ev of WORDED_EVENTS) expect(() => recordWords(rec(ev, { n: "x", ms: {}, error: 7, link: 3, host: [], code: null, bytes: "a", outcome: false })), ev).not.toThrow();
  });

  it("an event this build does not know is shown by name and fields", () => {
    expect(recordWords(rec("session.create", { session: "s-1", driver: "claude", ms: 40 }))).toBe("session.create — session s-1, driver claude, ms 40");
    expect(recordWords(rec("brand.new"))).toBe("brand.new");
    expect(recordWords(rec("constructor"))).toBe("constructor");
    expect(recordWords(rec("__proto__"))).toBe("__proto__");
  });

  it("the sign-in vocabulary of a host that needs the user: held hosts, repeated prompts, what is learned, why a prompt ended", () => {
    // Every value the daemon's vocabulary has (marsad-proto diag.rs: RepeatWord, LearnedWord, CancelWhy) reads as words, never raw.
    const prompt = (repeat?: string) => recordWords(rec("auth.prompt", { host: "gpu", prompt: 7, kind: "secret", attempt: 1, ...(repeat ? { repeat } : {}) }));
    expect(prompt()).toBe("Sign-in prompt 7 (secret), attempt 1");
    expect(prompt("helper_restart")).toBe("Sign-in prompt 7 (secret), attempt 1, a second approval within the same sign-in: the helper on the host was updated and restarted");
    expect(prompt("id_restart")).toContain("the host's daemon was restarted under this host's id");
    expect(prompt("windows_shell")).toContain("the host was connected again as a Windows machine");
    expect(prompt("something_new")).toContain("a second approval within the same sign-in: something_new");
    const learned = (what: string) => recordWords(rec("hosts.learned", { host: "gpu", what }));
    expect(learned("interactive")).toBe("Learned the host's sign-in: it asks you for something each time, so Marsad does not reconnect it on its own");
    expect(learned("not_interactive")).toBe("Learned the host's sign-in: it no longer asks you for anything, so Marsad reconnects it on its own again");
    expect(learned("machine")).toBe("Learned the host's machine identity");
    expect(learned("brand_new")).toBe("Learned the host's brand_new");
    const cancelled = (why: string) => recordWords(rec("auth.cancelled", { host: "gpu", prompt: 7, why }));
    expect(cancelled("attempt_ended")).toBe("Sign-in prompt 7 was cancelled: the connect attempt it belonged to ended");
    expect(cancelled("helper_gone")).toBe("Sign-in prompt 7 was cancelled: the ssh helper went away");
    expect(recordTone(rec("hosts.held", { host: "gpu" }))).toBe("warn");
  });

  it("tones", () => {
    expect(recordTone(rec("link.attempt_end", { outcome: "connected" }))).toBe("ok");
    expect(recordTone(rec("link.attempt_end", { outcome: "park" }))).toBe("bad");
    expect(recordTone(rec("boot.failed", {}, "warn"))).toBe("bad");
    expect(recordTone(rec("rpc.traced", { outcome: "error" }))).toBe("bad");
    expect(recordTone(rec("rpc.traced", { outcome: "ok" }))).toBe("plain");
    expect(recordTone(rec("link.state", { to: "connected" }))).toBe("ok");
    expect(recordTone(rec("link.state", { to: "auth_needed" }))).toBe("bad");
    expect(recordTone(rec("session.lost", {}, "warn"))).toBe("warn");
    expect(recordTone(rec("diag.panic", {}, "error"))).toBe("bad");
  });

  it("the expanded view has the header and every field", () => {
    const f = Object.fromEntries(recordFields(rec("boot.ok", { host: "hz", n: 1, ms: 38 })).map((x) => [x.k, x.v]));
    expect(f).toMatchObject({ source: "hub (boot b1)", seq: "1", level: "info", host: "hz", n: "1", ms: "38" });
  });

  it("a line that is not a record is not one", () => {
    expect(parseRecord("not json")).toBeNull();
    expect(parseRecord("[1]")).toBeNull();
    expect(parseRecord('{"ev":"x"}')).toBeNull();
    expect(parseRecord('{"diag_file":1,"gen":2}')).toBeNull();
    expect(parseRecord('{"ev":"x","seq":3,"t":4}')).toMatchObject({ id: ".3", ev: "x", lvl: "info", fields: {} });
  });
});

describe("app event words", () => {
  const events: [UiEvent, string][] = [
    [{ ev: "ui.prompt", trace: TRACE, host: "x1", session: "s", kind: "prompt", chars: 12, attachments: 1, bytes: 2048 }, "You sent a prompt (12 characters, 1 attachment, 2.0 KB)"],
    [{ ev: "ui.prompt", trace: TRACE, host: "x1", session: "s", kind: "slash", chars: 0, attachments: 0, bytes: 0 }, "You sent a slash command (0 characters)"],
    [{ ev: "ui.upload", trace: TRACE, files: 2, bytes: 4096, ms: 340, outcome: "error", code: "too_large" }, "Uploaded 2 files (4.0 KB) in 340 ms: error (too_large)"],
    [{ ev: "ui.command", trace: TRACE, host: "x1", session: "s", command: "answer_gate" }, "You sent answer gate"],
    [{ ev: "ui.launch", trace: TRACE, host: "hz", driver: "claude", resume: "continue", first_prompt: true, attachments: 2 }, "You launched a session on hz (claude, continue), with a first prompt and 2 attachments"],
    [{ ev: "ui.rpc", trace: TRACE, method: "session.command", ms: 12, outcome: "ok" }, "session.command was answered ok in 12 ms"],
    [{ ev: "ui.render_error", component: "Transcript" }, "The Transcript view failed to draw"],
    [{ ev: "ui.developer_mode", on: false }, "Developer mode was turned off"],
    [{ ev: "ui.export", stage: "save_cancelled" }, "Export diagnostics: save cancelled"],
  ];
  it.each(events)("%j", (e, words) => expect(appEventWords(e)).toBe(words));
  it("an event of another version is shown by name and fields", () => expect(appEventWords({ ev: "ui.teleport", to: "mars" } as unknown as UiEvent)).toBe("ui.teleport — to mars"));
});

describe("formatters", () => {
  it("durations", () => {
    expect([0, 12, 999, 1000, 1400, 12_300, 59_999, 60_000, 125_000].map(duration)).toEqual(["0 ms", "12 ms", "999 ms", "1.0 s", "1.4 s", "12 s", "60 s", "1 min 0 s", "2 min 5 s"]);
    expect(duration(-1500)).toBe("-1.5 s");
    expect(duration(NaN)).toBe("?");
  });
  it("sizes", () => expect([0, 1023, 1024, 15_000, 4_300_000, 52_428_800].map(sizeWords)).toEqual(["0 B", "1023 B", "1.0 KB", "15 KB", "4.1 MB", "50 MB"]));
  it("clock times have milliseconds", () => expect(clockMs(1_700_000_000_007)).toMatch(/^\d\d:\d\d:\d\d\.007$/));
});

describe("trail words: the earlier history of a resumed conversation", () => {
  const e = (over: Partial<Extract<TrailEntry, { ev: "earlier.history" }>>): TrailEntry => ({ ev: "earlier.history", state: "loaded", items: 140, omitted: 0, shortened: 0, unreadable: 0, unrecognised: 0, complete: true, interrupted: false, ...over });

  it.each<[string, Partial<Extract<TrailEntry, { ev: "earlier.history" }>>, string]>([
    ["loading", { state: "loading", items: 40, source: "agent_storage" }, "Earlier history is loading: 40 items so far, read from the agent's own storage"],
    ["loaded complete", { state: "loaded", items: 1 }, "Earlier history loaded: 1 item (complete)"],
    ["loaded with gaps", { state: "loaded", complete: false, omitted: 120, shortened: 2, unreadable: 1, unrecognised: 3, source: "agent_replay" }, "Earlier history loaded: 140 items (with gaps: 120 older omitted, 2 shortened, 1 unreadable, 3 not understood), replayed by the agent over its protocol"],
    ["interrupted", { state: "loaded", complete: false, interrupted: true }, "Earlier history loaded: 140 items (with gaps: interrupted)"],
    ["unavailable", { state: "unavailable", items: 0, reason: "source_missing", source: "agent_storage" }, "Earlier history is not available — the agent's file for the conversation does not exist, read from the agent's own storage"],
    ["unavailable after some", { state: "unavailable", items: 12, reason: "failed" }, "Earlier history is not available — the import failed; 12 items arrived first"],
    ["unavailable for a reason this build does not know", { state: "unavailable", items: 0, reason: "from_the_future" }, "Earlier history is not available — from_the_future"],
    ["none", { state: "none", items: 0 }, "There is no earlier history"],
    ["a state this build does not know", { state: "paused", items: 5 }, "The earlier history is in the state paused (5 items)"],
  ])("%s", (_name, over, words) => {
    expect(trailWords(e(over))).toBe(words);
  });

  it("is plain unless something is missing or it cannot come", () => {
    expect(trailTone(e({ state: "loading" }))).toBe("plain");
    expect(trailTone(e({}))).toBe("plain");
    expect(trailTone(e({ complete: false, omitted: 3 }))).toBe("warn");
    expect(trailTone(e({ state: "unavailable", reason: "too_large" }))).toBe("warn");
  });
});
