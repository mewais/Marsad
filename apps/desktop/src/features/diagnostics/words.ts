import type { TrailEntry, TrailItem, UiEvent } from "../../bridge/wire";
import type { DiagRecord } from "../../lib/diagRecord";

/**
 * Every sentence the Diagnostics panels show comes from this module: one pure function per kind of
 * thing (a trail entry, a daemon log record, an event of the app), with the raw fields available
 * for each. Nothing here reads anything but the entry it is given, so the words are as content-free
 * as the data (ids, sizes, timings, states, reasons). A kind this build does not know is shown
 * generically (its name and fields), never refused and never thrown on.
 */

export type Tone = "ok" | "warn" | "bad" | "info" | "plain";

// ── small formatters ────────────────────────────────────────────────────────

/** "12 ms", "1.4 s", "2 min 5 s". */
export function duration(ms: number): string {
  if (!Number.isFinite(ms)) return "?";
  const a = Math.abs(ms);
  const sign = ms < 0 ? "-" : "";
  if (a < 1000) return `${sign}${Math.round(a)} ms`;
  if (a < 60_000) return `${sign}${(a / 1000).toFixed(a < 10_000 ? 1 : 0)} s`;
  const m = Math.floor(a / 60_000);
  return `${sign}${m} min ${Math.round((a % 60_000) / 1000)} s`;
}

/** "512 B", "1.5 KB", "3.2 MB". */
export function sizeWords(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`;
  return `${(n / 1024 / 1024).toFixed(n < 10 * 1024 * 1024 ? 1 : 0)} MB`;
}

/** "14:03:22.481": a time of day with milliseconds, on the viewer's clock. */
export function clockMs(ms: number): string {
  const d = new Date(ms);
  const p = (n: number, w = 2) => String(n).padStart(w, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** What the daemons call an error: `{code, text?, len?, fp?, os?}`. Text only where it is the daemon's own scrubbed words. */
export function errorWords(e: unknown): string {
  if (typeof e === "string") return e;
  if (!e || typeof e !== "object") return "";
  const x = e as { code?: unknown; text?: unknown; len?: unknown; fp?: unknown; os?: unknown };
  const code = typeof x.code === "string" ? x.code : "error";
  if (typeof x.text === "string" && x.text) return `${code}: ${x.text}`;
  const bits = [typeof x.len === "number" ? `${plural(x.len, "character")} long` : "", typeof x.fp === "string" ? `fingerprint ${x.fp}` : "", typeof x.os === "number" ? `OS error ${x.os}` : ""].filter(Boolean);
  return bits.length ? `${code} (${bits.join(", ")}; the text itself is never recorded)` : code;
}

const say = (...parts: (string | false | null | undefined)[]) => parts.filter(Boolean).join("");
const num = (n: number | null | undefined, one: string, many?: string) => (typeof n === "number" ? plural(n, one, many) : "");

/** `key = value` pairs of an object, for the generic rendering and the expanded view. */
export function fieldPairs(o: Record<string, unknown>, skip: readonly string[] = []): { k: string; v: string }[] {
  return Object.entries(o)
    .filter(([k, v]) => !skip.includes(k) && v !== undefined && v !== null)
    .map(([k, v]) => ({ k, v: typeof v === "string" ? v : JSON.stringify(v) }));
}

/** The words for a kind this build has none for: its name and its fields. */
export function genericWords(name: string, o: Record<string, unknown>, skip: readonly string[] = []): string {
  const pairs = fieldPairs(o, skip);
  return pairs.length ? `${name} — ${pairs.map(({ k, v }) => `${k} ${v}`).join(", ")}` : name;
}

// ── the session trail ───────────────────────────────────────────────────────

const PHASE: Record<string, string> = { starting: "Starting", idle: "Idle", working: "Working", needs_you: "Needs you", exited: "Exited", failed: "Failed", lost: "Lost" };
const phaseWord = (to: string) => PHASE[to] ?? to;

const END_REASON: Record<string, string> = {
  cancelled: "cancelled",
  max_turns: "the turn limit was reached",
  execution_error: "the agent reported an error",
  compaction_needed: "the context needs compacting",
};

const GATE_HOW: Record<string, string> = { answered: "you answered it", resolved: "the agent or its protocol resolved it", withdrawn: "its owner withdrew it" };

type Cause = Extract<TrailEntry, { ev: "phase" }>["cause"];

/** Why the phase changed, as the end of a sentence ("Idle — the turn ended"). */
export function causeWords(c: Cause): string {
  switch (c.kind) {
    case "spawned":
      return `the process (generation ${c.generation}) was started`;
    case "handshake_done":
      return "the agent finished starting up";
    case "turn_started":
      return say("a turn started", typeof c.input === "number" && ` for input #${c.input}`);
    case "turn_completed":
      return c.interrupted ? "the turn was interrupted" : say("the turn ended", c.end_reason && ` (${END_REASON[c.end_reason] ?? c.end_reason})`);
    case "gate_raised":
      return c.external ? `a request from outside the agent is waiting for your decision (${c.request_id})` : `a request is waiting for your decision (${c.request_id})`;
    case "gate_cleared":
      return `request ${c.request_id} is no longer pending (${GATE_HOW[c.how] ?? c.how})`;
    case "exited":
      return say("the process exited", typeof c.code === "number" && ` with code ${c.code}`, typeof c.signal === "number" && ` on signal ${c.signal}`, c.requested && " after a stop was requested", c.handshake_timeout && " (the startup deadline had passed)");
    case "lost":
      return "the host lost track of the process";
    case "other":
      return "no more specific cause was recorded";
    default:
      return genericWords((c as { kind: string }).kind, c as Record<string, unknown>, ["kind"]);
  }
}

const DROP: Record<string, string> = {
  interrupted: "it was interrupted while it waited",
  shutdown: "the session was shut down first",
  unencodable: "it could not be encoded for the agent",
  refused: "the agent refused it",
  process_ended: "the process ended before the agent took it",
};
const STATUS: Record<string, string> = {
  sent: "handed to the agent",
  queued: "held until the agent can take it",
  unsupported: "this agent cannot do that",
  rejected: "rejected",
  not_running: "the session was not running",
  withdrawn: "the request it answers is gone",
};
const NOTICE: Record<string, string> = {
  stop_requested: "A graceful stop was requested",
  handshake_deadline: "The startup deadline passed before the agent was ready",
  signal_sent: "A signal was sent to the process",
  previous_generation_unclosed: "The previous process was still open when a new one started",
  start_ignored: "A start request was ignored",
  shim: "The process supervisor reported something",
  unrecognised_note: "A journal note was not recognised",
  other: "The runtime noted something",
};
const RESUME: Record<string, string> = { fresh: "a new conversation", continue: "continuing the latest conversation", resume: "resuming a conversation" };

const input = (n: number | null | undefined) => (typeof n === "number" ? `input #${n}` : "an input");
const Input = (n: number | null | undefined) => `${input(n)[0].toUpperCase()}${input(n).slice(1)}`;

function appliedWords(e: Extract<TrailEntry, { ev: "input.applied" }>): string {
  const files = e.attachments ? `, ${plural(e.attachments, "attachment")} (${sizeWords(e.bytes)})` : "";
  const what: Record<string, string> = {
    prompt: `A prompt was sent (${plural(e.chars, "character")}${files})`,
    run_slash: `Slash command /${e.target ?? "?"} was run (${plural(e.chars, "character")} of arguments)`,
    answer_gate: `A request (${e.target ?? "?"}) was answered${e.choice ? ` with ${e.choice}` : ""}${e.chars ? ` and ${plural(e.chars, "character")} of text` : ""}`,
    answer_question: `A question (${e.target ?? "?"}) was answered (${plural(e.chars, "character")})`,
    interrupt: "An interrupt was sent",
    set_model: `The model was set to ${e.target ?? "?"}`,
    set_mode: `The mode was set to ${e.target ?? "?"}`,
    shutdown: "A shutdown was sent",
  };
  const head = what[e.kind] ?? `A command (${e.kind}) was applied`;
  return `${head} — #${e.input}, ${STATUS[e.status] ?? e.status}${e.reason ? ` (${errorWords(e.reason)})` : ""}`;
}

/** The sentence of one trail entry. The only place that words a `TrailEntry`. */
export function trailWords(e: TrailEntry): string {
  switch (e.ev) {
    case "process.spawned":
      return say("A process was started (generation ", String(e.generation), typeof e.pid === "number" && `, pid ${e.pid}`, `), ${plural(e.argc, "argument")}`, e.flags.length > 0 && `, flags ${e.flags.join(" ")}`);
    case "process.started":
      return say(`Start (generation ${e.generation}): ${RESUME[e.resume] ?? e.resume}`, e.resume_id && ` ${e.resume_id}`, e.model && `, model ${e.model}`, e.mode && `, mode ${e.mode}`, typeof e.first_input === "number" && `, with a first prompt (input #${e.first_input})`);
    case "agent.init":
      return say("The agent introduced itself", e.native_session_id && ` — its session ${e.native_session_id}`, e.cli_version && `, version ${e.cli_version}`, e.model && `, model ${e.model}`, e.mode && `, mode ${e.mode}`, `; ${plural(e.tools, "tool")}, ${plural(e.mcp_servers, "MCP server")}, ${plural(e.slash_commands, "slash command")}`);
    case "input.applied":
      return appliedWords(e);
    case "input.sent":
      return `${Input(e.input)} left the queue and is being written to the agent`;
    case "input.written":
      return `${Input(e.input)} was written to the agent's input`;
    case "input.write_failed":
      return `Writing to the agent failed${typeof e.input === "number" ? ` for input #${e.input}` : ""} — ${errorWords(e.error)}`;
    case "input.acknowledged":
      return `The agent acknowledged ${input(e.input)}${e.steered ? " (steered into the running turn)" : ""}`;
    case "input.dropped":
      return say(`${Input(e.input)} will never reach the agent — ${DROP[e.reason] ?? e.reason}`, e.error && ` (${errorWords(e.error)})`);
    case "turn.started":
      return say("A turn started", typeof e.input === "number" && ` for input #${e.input}`, e.native_turn_id && ` (turn ${e.native_turn_id})`);
    case "turn.completed":
      return say(
        e.interrupted ? "The turn was interrupted" : e.end_reason ? `The turn ended (${END_REASON[e.end_reason] ?? e.end_reason})` : "The turn ended",
        `: ${plural(e.num_turns, "turn")}, ${e.input_tokens} tokens in, ${e.output_tokens} out`,
        e.cost_usd_micros > 0 && `, $${(e.cost_usd_micros / 1e6).toFixed(4)}`,
        `, a reply of ${plural(e.reply_chars, "character")}`,
        e.error && ` — ${errorWords(e.error)}`,
      );
    case "tool.started":
      return `Tool ${e.name} started${e.subagent ? " (a subagent)" : ""} — call ${e.call_id}`;
    case "tool.finished":
      return `Tool call ${e.call_id} ${e.ok ? "finished" : "failed"}`;
    case "gate.raised":
      return say(`Waiting for you — the agent asked for a ${e.gate_type} decision`, e.tool && ` on ${e.tool}`, e.choices > 0 && ` (${plural(e.choices, "choice")})`, ` — request ${e.request_id}`);
    case "gate.resolved":
      return say(`Request ${e.request_id} is no longer pending`, e.reason && ` (${e.reason})`);
    case "agent.model":
      return `The model is now ${e.model}`;
    case "agent.mode":
      return `The mode is now ${e.mode}`;
    case "agent.rejected":
      return `The agent rejected a command (${e.command}) — ${errorWords(e.error)}`;
    case "agent.notice":
      return `The agent sent a notice — ${errorWords(e.error)}`;
    case "agent.compaction":
      return say(`The conversation was compacted: ${e.pre_tokens} → ${e.post_tokens} tokens in ${duration(e.duration_ms)}`, e.trigger && ` (${e.trigger})`);
    case "agent.rate_limited":
      return say("The provider reports a rate limit", e.status && `: ${e.status}`, e.rate_limit_type && ` (${e.rate_limit_type})`, e.is_using_overage && ", using overage", e.resets_at > 0 && `, resets at ${new Date(e.resets_at * 1000).toISOString().slice(11, 19)} UTC`);
    case "agent.unknown":
      return `The agent sent something Marsad does not recognise (${e.raw_type})`;
    case "process.stderr":
      return `The agent wrote ${plural(e.chars, "character")} to its error output`;
    case "phase":
      return say(`${phaseWord(e.to)} — ${causeWords(e.cause)}`, e.reason && `: ${e.reason}`);
    case "process.exited":
      return say("The process exited", typeof e.code === "number" && ` with code ${e.code}`, typeof e.signal === "number" && ` on signal ${e.signal}`);
    case "process.lost":
      return `The host lost the process — ${e.reason}`;
    case "runtime.notice":
      return NOTICE[e.code] ?? `The runtime noted something (${e.code})`;
    case "anomaly":
      return e.kind === "activity_while_idle" ? `The agent produced output after its turn had ended (first: ${e.first})` : `Something did not fit the session's state (${e.kind}, first: ${e.first})`;
    default: {
      const u = e as { ev?: unknown } & Record<string, unknown>;
      return genericWords(typeof u.ev === "string" ? u.ev : "an unknown trail entry", u, ["ev", "cursor", "t"]);
    }
  }
}

/** How much attention a trail entry deserves. */
export function trailTone(e: TrailEntry): Tone {
  switch (e.ev) {
    case "input.write_failed":
    case "input.dropped":
    case "agent.rejected":
    case "process.lost":
      return "bad";
    case "anomaly":
    case "agent.notice":
    case "agent.rate_limited":
    case "agent.unknown":
    case "gate.raised":
      return "warn";
    case "phase":
      return e.to === "failed" || e.to === "lost" ? "bad" : e.to === "needs_you" ? "warn" : "plain";
    case "process.exited":
      return e.code === 0 || e.code === undefined || e.code === null ? "plain" : "bad";
    case "turn.completed":
      return e.error ? "bad" : "plain";
    case "input.applied":
      return e.status === "sent" || e.status === "queued" ? "plain" : "bad";
    case "input.acknowledged":
      return "ok";
    default:
      return "plain";
  }
}

/** All the fields of a trail item, as the host sent them (the expanded view). */
export function trailFields(it: TrailItem): { k: string; v: string }[] {
  const { cursor, t, ...entry } = it as TrailItem & Record<string, unknown>;
  return [{ k: "cursor", v: String(cursor) }, { k: "time", v: `${clockMs(t)} (${t})` }, ...fieldPairs(entry)];
}

// ── daemon log records ──────────────────────────────────────────────────────

type F = Record<string, unknown>;
const str = (f: F, k: string): string | undefined => (typeof f[k] === "string" && f[k] ? (f[k] as string) : undefined);
const nm = (f: F, k: string): number | undefined => (typeof f[k] === "number" ? (f[k] as number) : undefined);
const flag = (f: F, k: string): boolean => f[k] === true;
const ms = (f: F, k = "ms"): string => (typeof f[k] === "number" ? duration(f[k] as number) : "?");
const bytes = (f: F, k = "bytes"): string => (typeof f[k] === "number" ? sizeWords(f[k] as number) : "?");
const err = (f: F, k = "error"): string => (f[k] === undefined || f[k] === null ? "" : errorWords(f[k]));
const tail = (s: string) => (s ? ` — ${s}` : "");
/** The host a link label names (`hosts:gpu` → `gpu`). */
const linkHost = (f: F): string => (str(f, "link") ?? "a link").replace(/^hosts:/, "");
const at = (f: F) => (typeof f.n === "number" ? `attempt ${f.n}` : "an attempt");
const exit = (f: F, k = "exit") => (typeof f[k] === "number" ? `exit code ${f[k]}` : "");

const OUTCOME: Record<string, string> = {
  connected: "connected",
  backoff: "failed; the next attempt follows the backoff schedule",
  park: "failed; it waits for you (no automatic retry)",
  provision: "found the helper must be installed or updated",
  immediate: "failed; it will be retried at once",
};
const CLOSED: Record<string, string> = {
  eof: "the other side closed it",
  goodbye: "the other side said goodbye",
  io: "an I/O error",
  protocol: "a protocol error",
  frame_too_large: "a frame was too large",
  idle_timeout: "it was idle too long",
  local: "this side closed it",
};
const WHY: Record<string, string> = { mingw_sh: "a POSIX shell answered on Windows", no_report: "the first try gave no report" };
const PROMPT_CANCEL: Record<string, string> = { user: "you cancelled it", timeout: "it timed out", superseded: "a newer prompt replaced it", helper_gone: "the ssh helper went away" };
const BRIDGE: Record<string, string> = { running: "found the daemon already running", started: "started the daemon", start_failed: "could not start the daemon", unavailable: "could not reach the daemon" };
const SHUTDOWN: Record<string, string> = { signal: "a signal", request: "a request to stop", serve_ended: "it stopped serving", serve_failed: "serving failed" };

/**
 * One entry per event name worded specially. Every one takes the record's own fields and must cope
 * with any of them missing (a daemon of another version): it never throws.
 */
const RECORD_WORDS: Record<string, (f: F) => string> = {
  // Connection trail (the hub's link to a host)
  "link.attempt_begin": (f) => `Connecting to ${linkHost(f)} — ${at(f)} began`,
  "link.attempt_end": (f) => `${at(f)[0].toUpperCase()}${at(f).slice(1)} ${OUTCOME[str(f, "outcome") ?? ""] ?? str(f, "outcome") ?? "ended"} after ${ms(f)}${tail(err(f))}`,
  "link.state": (f) => say(`${linkHost(f)} is ${str(f, "to") ?? "?"}`, str(f, "from") && ` (was ${str(f, "from")})`, typeof f.attempt === "number" && `, attempt ${f.attempt}`, tail(str(f, "error") ?? "")),
  "link.provision": (f) => `Helper on ${linkHost(f)} (${str(f, "reason") ?? "?"}): ${str(f, "outcome") ?? "?"} after ${ms(f)}${tail(err(f))}`,
  "link.closed": (f) => say(`The connection to ${linkHost(f)} closed (${CLOSED[str(f, "reason") ?? ""] ?? str(f, "reason") ?? "?"}) after being up ${ms(f, "connected_ms")}`, typeof f.transport_exit === "number" && `, the transport exited with code ${f.transport_exit}`, tail(str(f, "stderr") ?? "")),
  "transport.spawned": (f) => say(`Started ${str(f, "program") ?? "the transport"} for ${linkHost(f)} (${at(f)}; ${num(nm(f, "argc"), "argument")}`, typeof f.pid === "number" && `, pid ${f.pid}`, ")"),
  "transport.exited": (f) => say(`The transport to ${linkHost(f)} exited (${at(f)})`, typeof f.code === "number" && ` with code ${f.code}`, typeof f.signal === "number" && ` on signal ${f.signal}`, `; ${bytes(f, "stderr_bytes")} of error output`),
  "conn.accepted": (f) => `A peer connected (role ${str(f, "role") ?? "?"}, build ${str(f, "peer_build") ?? "?"}, host ${str(f, "peer_host") ?? "?"})`,
  "conn.handshake_failed": (f) => `A connection failed its handshake${tail(err(f))}`,
  "conn.closed": (f) => `A peer connection closed (role ${str(f, "role") ?? "?"}, ${str(f, "reason") ?? "?"}) after ${ms(f)}`,
  "rpc.traced": (f) => say(`${str(f, "method") ?? "A request"} ${str(f, "host") ? `for ${str(f, "host")} ` : ""}(${num(nm(f, "hops"), "hop")}) ${str(f, "outcome") ?? "ended"} in ${ms(f)}`, str(f, "code") && ` (${str(f, "code")})`),
  "rpc.forward_failed": (f) => `Could not forward ${str(f, "method") ?? "a request"} to ${str(f, "host") ?? "a host"} — ${str(f, "code") ?? "failed"} after ${ms(f)}`,
  "sub.closed": (f) => `A ${str(f, "topic") ?? "topic"} subscription${str(f, "host") ? ` on ${str(f, "host")}` : ""} ended: ${str(f, "reason") ?? "?"}${str(f, "code") ? ` (${str(f, "code")})` : ""}`,
  // Bootstrap of a host
  "boot.attempt": (f) => say(`Bootstrap ${at(f)}: ${str(f, "shell") ?? "?"} shell`, flag(f, "first_contact") && ", first contact", flag(f, "interactive") && ", sign-in can be asked", flag(f, "armed") && ", armed"),
  "boot.preamble": (f) => `The login shell printed ${num(nm(f, "lines"), "line")} (${bytes(f)}) before the bootstrap${tail(str(f, "shape") ?? "")}`,
  "boot.report": (f) => `The host reports ${str(f, "os") ?? "?"}/${str(f, "arch") ?? "?"}, helper ${str(f, "installed") ?? "not installed"} (${ms(f)})`,
  "boot.windows_retry": (f) => `Trying again for Windows: ${WHY[str(f, "why") ?? ""] ?? str(f, "why") ?? "?"}`,
  "boot.action": (f) => say(`Helper: ${str(f, "action") ?? "?"}`, typeof f.bytes === "number" && ` (${sizeWords(f.bytes)})`, str(f, "target") && ` → ${str(f, "target")}`),
  "boot.uploaded": (f) => `Uploaded the helper (${bytes(f)}) in ${ms(f)}`,
  "boot.ok": (f) => `The helper started on the host in ${ms(f)}`,
  "boot.hello": (f) => say(`The helper answered (build ${str(f, "build") ?? "?"}) in ${ms(f)}`, f.host_id_matches === false && ", but under another host id", str(f, "machine") && `, machine ${str(f, "machine")}`),
  "boot.failed": (f) => say(`Bootstrap failed at ${str(f, "stage") ?? "?"}`, str(f, "kind") && ` (${str(f, "kind")})`, exit(f) && `, ${exit(f)}`, tail(str(f, "stderr") ?? ""), tail(err(f))),
  "boot.twin": (f) => `This host reaches the same machine as ${str(f, "twin") ?? "another host"}`,
  "boot.id_mismatch": (f) => `The machine answered as host ${str(f, "theirs") ?? "?"}, not as this one`,
  "boot.build_mismatch": (f) => `The helper is build ${str(f, "found") ?? "?"}; ${str(f, "expected") ?? "?"} is expected`,
  // Sign-in, host registry, host settings, session feed
  "auth.prompt": (f) => `Sign-in prompt ${f.prompt ?? "?"} (${str(f, "kind") ?? "?"}), attempt ${f.attempt ?? "?"}`,
  "auth.answered": (f) => `Sign-in prompt ${f.prompt ?? "?"} was answered after ${ms(f)}`,
  "auth.cancelled": (f) => `Sign-in prompt ${f.prompt ?? "?"} was cancelled: ${PROMPT_CANCEL[str(f, "why") ?? ""] ?? str(f, "why") ?? "?"}`,
  "hosts.changed": (f) => say(`Host ${str(f, "op") ?? "changed"} (${str(f, "kind") ?? "?"}`, str(f, "target_kind") && `, target ${str(f, "target_kind")}`, ")"),
  "hosts.learned": (f) => `Learned the host's ${str(f, "what") ?? "?"}`,
  "hosts.failed": (f) => `The host registry failed at ${str(f, "op") ?? "?"}${tail(err(f))}`,
  "hostenv.apply": (f) => say(`Host settings ${str(f, "outcome") ?? "?"}`, str(f, "revision") && ` (revision ${str(f, "revision")})`, ` in ${ms(f)}`, f.setup_ok === false && ", the setup commands failed", tail(err(f))),
  "fleet.feed": (f) => `The session feed of ${str(f, "host") ?? "a host"} ${str(f, "event") ?? "changed"}${tail(err(f))}`,
  launch: (f) => say(`Launch of ${str(f, "driver") ?? "an agent"} on ${str(f, "host") ?? "a host"} (${str(f, "resume") ?? "?"}) ${str(f, "outcome") ?? "ended"} in ${ms(f)}`, str(f, "session") && ` — session ${str(f, "session")}`, flag(f, "first_prompt") && ", with a first prompt", tail(err(f))),
  // Daemon, bridge, helpers
  "daemon.start": (f) => `Daemon started (${flag(f, "hub") ? "hub" : "host"} ${str(f, "host_id") ?? "?"}, build ${str(f, "build") ?? "?"}, pid ${f.pid ?? "?"})`,
  "daemon.ready": (f) => `Daemon ready in ${ms(f, "total_ms")} (environment ${ms(f, "env_ms")}, adopting sessions ${ms(f, "adopt_ms")}, node ${ms(f, "node_ms")})`,
  "daemon.readiness_unwritten": (f) => `The daemon could not tell its starter it was ready (${str(f, "why") ?? "?"})${tail(err(f))}`,
  "daemon.lock_held": () => "Another daemon holds this machine's lock",
  "daemon.failed": (f) => `The daemon failed at ${str(f, "stage") ?? "?"}${typeof f.exit === "number" ? ` (exit code ${f.exit})` : ""}${tail(err(f))}`,
  "daemon.shutdown": (f) => say(`Daemon shut down: ${SHUTDOWN[str(f, "why") ?? ""] ?? str(f, "why") ?? "?"}`, str(f, "signal") && ` (${str(f, "signal")})`, tail(err(f))),
  "bridge.start": (f) => `The bridge started for ${str(f, "host_id") ?? "a host"} (pid ${f.pid ?? "?"})`,
  "bridge.daemon": (f) => say(`The bridge ${BRIDGE[str(f, "outcome") ?? ""] ?? str(f, "outcome") ?? "reports"}`, typeof f.ms === "number" && ` (${duration(f.ms)})`, str(f, "reason") && `: ${str(f, "reason")}`, typeof f.exit === "number" && `, exit code ${f.exit}`, tail(err(f))),
  "bridge.end": (f) => `The bridge ended (${str(f, "reason") ?? "?"})${tail(err(f))}`,
  "stop.end": (f) => `Stopping the daemon: ${str(f, "outcome") ?? "?"} in ${ms(f)}${tail(err(f))}`,
  "askpass.end": (f) => `The sign-in helper ended: ${str(f, "outcome") ?? "?"} in ${ms(f)}${tail(err(f))}`,
  // The host's own daemon (runtime)
  "session.command_refused": (f) => `The ${str(f, "kind") ?? "command"} for session ${str(f, "session") ?? "?"} was refused before it was recorded (${str(f, "code") ?? "?"})`,
  "session.command_timeout": (f) => `The ${str(f, "kind") ?? "command"} for session ${str(f, "session") ?? "?"} timed out waiting; it may still have been applied`,
  "env.capture": (f) => `Captured the login environment (${str(f, "source") ?? "?"}) in ${ms(f)}`,
  "env.setup": (f) => `The host's setup commands ${f.ok === false ? "failed" : "ran"} in ${ms(f)}`,
  "session.lost": (f) => `Session ${str(f, "session") ?? "?"} lost its process${tail(err(f))}`,
  "session.retry": (f) => `Reconnecting to the process of session ${str(f, "session") ?? "?"} — attempt ${f.attempt ?? "?"}${tail(err(f))}`,
  "session.op_failed": (f) => `${str(f, "op") ?? "An operation"} on session ${str(f, "session") ?? "?"} failed${tail(err(f))}`,
  "session.outbox_overflow": (f) => `Session ${str(f, "session") ?? "?"} fell too far behind its process`,
  "runtime.adopt_failed": (f) => `Session ${str(f, "session") ?? "?"} could not be taken over at start${tail(err(f))}`,
  // The sink itself
  "process.start": (f) => `Process started (${str(f, "role") ?? "?"}, build ${str(f, "build") ?? "?"}, pid ${f.pid ?? "?"})`,
  "process.exit": (f) => `Process exiting (${str(f, "reason") ?? "?"})`,
  "diag.dropped": (f) => `${num(nm(f, "count"), "diagnostics record")} could not be written (sequence ${f.first_seq ?? "?"}–${f.last_seq ?? "?"})`,
  "diag.suppressed": (f) => `${num(nm(f, "count"), "record")} of ${str(f, "event") ?? "one kind"} left out: too many in a minute`,
  "diag.write_failed": (f) => `The log file could not be written${tail(err(f))}`,
  "diag.panic": (f) => `A panic at ${str(f, "location") ?? "?"} on thread ${str(f, "thread") ?? "?"}`,
  "trace.counted": (f) => `${num(nm(f, "n"), "message")} at ${str(f, "level") ?? "?"} from ${str(f, "target") ?? "?"} (counted, not recorded)`,
};

/** The event names `recordWords` words specially (the rest are shown generically). */
export const WORDED_EVENTS: readonly string[] = Object.keys(RECORD_WORDS);

/** The sentence of one daemon log record. */
export function recordWords(r: DiagRecord): string {
  const w = Object.prototype.hasOwnProperty.call(RECORD_WORDS, r.ev) ? RECORD_WORDS[r.ev] : undefined;
  if (!w) return genericWords(r.ev, r.fields);
  try {
    return w(r.fields);
  } catch {
    // A field of an unexpected shape: say what the record is rather than nothing.
    return genericWords(r.ev, r.fields);
  }
}

/** How much attention a daemon log record deserves. */
export function recordTone(r: DiagRecord): Tone {
  const f = r.fields;
  if (r.lvl === "error" || r.ev === "boot.failed" || r.ev === "daemon.failed") return "bad";
  if (r.ev === "rpc.traced" && f.outcome !== "ok") return "bad";
  if (r.ev === "link.attempt_end") return f.outcome === "connected" ? "ok" : "bad";
  if (r.ev === "boot.ok" || r.ev === "boot.hello" || r.ev === "auth.answered") return "ok";
  if (r.ev === "link.state") return f.to === "connected" ? "ok" : f.to === "failed" || f.to === "auth_needed" || f.to === "incompatible" ? "bad" : "plain";
  if (r.lvl === "warn") return "warn";
  return "plain";
}

/** All the fields of a record, as written (the expanded view): the header, then the event's own. */
export function recordFields(r: DiagRecord): { k: string; v: string }[] {
  return [{ k: "source", v: `${r.src}${r.b ? ` (boot ${r.b})` : ""}` }, { k: "seq", v: String(r.seq) }, { k: "level", v: r.lvl }, { k: "time", v: `${clockMs(r.t)} (${r.t})` }, { k: "monotonic", v: duration(r.m) }, ...fieldPairs(r.fields)];
}

// ── the app's own events ────────────────────────────────────────────────────

/** The sentence of one event the app recorded of its own acts. */
export function appEventWords(e: UiEvent): string {
  switch (e.ev) {
    case "ui.prompt":
      return say(`You sent a ${e.kind === "slash" ? "slash command" : "prompt"} (${plural(e.chars, "character")}`, e.attachments > 0 && `, ${plural(e.attachments, "attachment")}, ${sizeWords(e.bytes)}`, ")");
    case "ui.upload":
      return say(`Uploaded ${plural(e.files, "file")} (${sizeWords(e.bytes)}) in ${duration(e.ms)}: ${e.outcome}`, e.code && ` (${e.code})`);
    case "ui.command":
      return `You sent ${e.command.replace(/_/g, " ")}`;
    case "ui.launch":
      return say(`You launched a session on ${e.host} (${e.driver}, ${e.resume})`, e.first_prompt && ", with a first prompt", e.attachments > 0 && ` and ${plural(e.attachments, "attachment")}`);
    case "ui.rpc":
      return say(`${e.method} was answered ${e.outcome} in ${duration(e.ms)}`, e.code && ` (${e.code})`);
    case "ui.render_error":
      return `The ${e.component} view failed to draw`;
    case "ui.developer_mode":
      return `Developer mode was turned ${e.on ? "on" : "off"}`;
    case "ui.export":
      return say(`Export diagnostics: ${e.stage.replace(/_/g, " ")}`, e.code && ` (${e.code})`);
    default: {
      const u = e as { ev?: unknown } & Record<string, unknown>;
      return genericWords(typeof u.ev === "string" ? u.ev : "an unknown app event", u, ["ev"]);
    }
  }
}
