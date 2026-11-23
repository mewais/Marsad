import type { Attachment, Command, CommandStatus, CompactionInfo, ContentPart, InputDropReason, InputId, RateLimitInfo, SessionEvent } from "../../bridge/wire";
import { clockTime, compactNumber } from "../../lib/format";

/** Why a message the user wrote did not reach the agent. */
export type NotSentCause = InputDropReason | Exclude<CommandStatus["status"], "sent" | "queued"> | "write_failed";

/**
 * Where a user message is on its way to the agent, as the daemon reported it: `queued` until it
 * leaves the driver's queue, `sent` once written, `not_sent` when the daemon says it never will be.
 */
export type Delivery = { state: "sent" } | { state: "queued" } | { state: "not_sent"; cause: NotSentCause; detail?: string };

export type UserItem = { kind: "user"; key: string; input?: InputId; text: string; delivery: Delivery; attachments: Attachment[] };
export type ThinkingItem = { kind: "thinking"; key: string; text: string };
export type ToolItem = { kind: "tool"; key: string; callId: string; name: string; input: unknown; subagent: boolean; done: boolean; ok?: boolean; result?: unknown; status?: string; parts: ContentPart[] };
/** `warn`: the agent or its provider says something to watch (a rate limit), not a failure. */
export type NoticeItem = { kind: "notice"; key: string; text: string; tone: "info" | "warn" | "err" };

/**
 * One folded transcript entry. Items are immutable: a change replaces the item with a new object,
 * so an item that did not change keeps its identity from one fold to the next.
 */
export type Item =
  | UserItem
  | { kind: "assistant"; key: string; text: string }
  /** Non-text content: an image or file the agent showed, or one the CLI reported in a user message. */
  | { kind: "media"; key: string; role: "assistant" | "user"; parts: ContentPart[] }
  | ThinkingItem
  | ToolItem
  | NoticeItem;

const SENT: Delivery = { state: "sent" };
const QUEUED: Delivery = { state: "queued" };
const NO_ATTACHMENTS: Attachment[] = [];

function deliveryOf(s: CommandStatus): Delivery {
  switch (s.status) {
    case "sent":
      return SENT;
    case "queued":
      return QUEUED;
    case "unsupported":
      return { state: "not_sent", cause: "unsupported", detail: s.operation };
    case "rejected":
    case "not_running":
      return { state: "not_sent", cause: s.status, detail: s.reason };
    case "withdrawn":
      return { state: "not_sent", cause: "withdrawn" };
  }
}

/** What a command is called when it could not be delivered and has no transcript entry of its own. */
const COMMAND_WORD: Partial<Record<Command["command"], string>> = {
  answer_gate: "Your answer",
  answer_question: "Your answer",
  interrupt: "The interrupt",
  set_model: "The model change",
  set_mode: "The mode change",
  shutdown: "The stop request",
};

/** The fold's working state: everything needed to continue from the last processed event. */
interface State {
  items: Item[];
  /** Index of each tool call's item, by call id. */
  tools: Map<string, number>;
  /** Index of each user message's item, by the input id the daemon gave it. */
  users: Map<InputId, number>;
  /** Other commands (answers, interrupt ...), by input id: what to call them if one is dropped. */
  commands: Map<InputId, string>;
  lastMsg: string | undefined;
  /** Events processed so far (a prefix of the events array). */
  count: number;
  dirty: boolean;
}

const blank = (): State => ({ items: [], tools: new Map(), users: new Map(), commands: new Map(), lastMsg: undefined, count: 0, dirty: false });

const push = (st: State, item: Item) => {
  st.items.push(item);
  st.dirty = true;
};
const replace = (st: State, i: number, item: Item) => {
  st.items[i] = item;
  st.dirty = true;
};

/** A real input id (`0` means "unknown", an event from a peer that predates ids). */
const inputId = (id: unknown): InputId | undefined => (typeof id === "number" && id > 0 ? id : undefined);

function userItem(st: State, key: string, input: InputId | undefined, text: string, delivery: Delivery, attachments: Attachment[]) {
  if (input) st.users.set(input, st.items.length);
  push(st, { kind: "user", key, input, text, delivery, attachments });
}

const sameDelivery = (a: Delivery, b: Delivery) => a === b || (a.state === "not_sent" && b.state === "not_sent" && a.cause === b.cause && a.detail === b.detail);

/** Change the delivery of the user message `input`; false when there is no such message. */
function deliver(st: State, input: InputId | undefined, next: (cur: Delivery) => Delivery): boolean {
  const i = input ? st.users.get(input) : undefined;
  const it = i === undefined ? undefined : st.items[i];
  if (i === undefined || it?.kind !== "user") return false;
  const delivery = next(it.delivery);
  if (!sameDelivery(delivery, it.delivery)) replace(st, i, { ...it, delivery });
  return true;
}

const notice = (st: State, key: string, text: string, tone: NoticeItem["tone"]) => push(st, { kind: "notice", key, text, tone });

/** Fold one event into the state. Kinds this build does not know are ignored, never an error. */
function apply(st: State, e: SessionEvent) {
  const key = String(e.cursor);
  const items = st.items;
  switch (e.kind) {
    case "input": {
      const c = e.command;
      const id = inputId(e.input);
      if (c.command === "prompt") userItem(st, key, id, c.text, deliveryOf(e.status), c.attachments ?? NO_ATTACHMENTS);
      else if (c.command === "run_slash") userItem(st, key, id, `/${c.name} ${c.args}`.trim(), deliveryOf(e.status), NO_ATTACHMENTS);
      else if (id && COMMAND_WORD[c.command]) st.commands.set(id, COMMAND_WORD[c.command]!);
      st.lastMsg = undefined;
      return;
    }
    case "write_failed": {
      const id = inputId(e.input);
      const detail = e.error;
      if (!deliver(st, id, () => ({ state: "not_sent", cause: "write_failed", detail }))) notice(st, key, `Writing to the agent failed: ${detail}`, "err");
      return;
    }
    case "notice":
      notice(st, key, e.message, "info");
      return;
    case "lost":
      notice(st, key, `Session lost: ${e.reason}`, "err");
      return;
    case "phase":
      if (e.phase.state === "failed") notice(st, key, `Failed: ${e.phase.reason}`, "err");
      return;
    case "agent":
      break;
    default:
      return;
  }
  const a = e.event;
  if (!a) return;
  switch (a.type) {
    case "assistant_delta":
    case "thinking_delta": {
      const kind = a.type === "assistant_delta" ? "assistant" : "thinking";
      const prev = items[items.length - 1];
      if (prev && prev.kind === kind && (a.message_id === undefined || a.message_id === st.lastMsg)) replace(st, items.length - 1, { ...prev, text: prev.text + a.text });
      else push(st, { kind, key, text: a.text });
      st.lastMsg = a.message_id ?? undefined;
      return;
    }
    case "tool_call_started":
      st.tools.set(a.call_id, items.length);
      push(st, { kind: "tool", key, callId: a.call_id, name: a.name, input: a.input, subagent: a.is_subagent, done: false, parts: [] });
      st.lastMsg = undefined;
      return;
    case "tool_call_updated": {
      const i = st.tools.get(a.call_id) ?? -1;
      const t = items[i];
      if (t?.kind !== "tool") return;
      const status = a.status ?? undefined;
      // ACP agents send the arguments after announcing the call.
      const input = a.input != null ? a.input : t.input;
      const parts = a.parts?.length ? [...t.parts, ...a.parts] : t.parts;
      if (status !== t.status || input !== t.input || parts !== t.parts) replace(st, i, { ...t, status, input, parts });
      return;
    }
    case "tool_call_finished": {
      const i = st.tools.get(a.call_id) ?? -1;
      const t = items[i];
      if (t?.kind !== "tool") return;
      const parts = a.parts?.length ? a.parts : t.parts;
      if (!t.done || t.ok !== a.ok || !Object.is(t.result, a.result) || parts !== t.parts) replace(st, i, { ...t, done: true, ok: a.ok, result: a.result, parts });
      return;
    }
    case "assistant_content":
    case "user_content": {
      const role = a.type === "assistant_content" ? "assistant" : "user";
      const prev = items[items.length - 1];
      if (prev?.kind === "media" && prev.role === role) replace(st, items.length - 1, { ...prev, parts: [...prev.parts, ...a.parts] });
      else push(st, { kind: "media", key, role, parts: a.parts });
      st.lastMsg = undefined;
      return;
    }
    case "input_sent":
      // Leaves the driver's queue: only a queued message changes.
      deliver(st, inputId(a.input), (cur) => (cur.state === "queued" ? SENT : cur));
      return;
    case "input_dropped": {
      const id = inputId(a.input);
      const detail = a.error ?? undefined;
      const reason = a.reason;
      // A message the daemon already gave up on keeps its first verdict.
      if (deliver(st, id, (cur) => (cur.state === "not_sent" ? cur : { state: "not_sent", cause: reason, detail }))) return;
      const what = id ? st.commands.get(id) : undefined;
      if (what) notice(st, key, `${what} was not sent: ${dropWords(reason)}${detail ? ` (${detail})` : ""}`, reason === "unencodable" || reason === "refused" ? "err" : "info");
      return;
    }
    case "notice":
      notice(st, key, a.message, "info");
      return;
    case "compaction":
      notice(st, key, compactionWords(a.info, e.t), "info");
      return;
    case "rate_limit_notice":
      notice(st, key, rateLimitWords(a.info), "warn");
      return;
    case "command_rejected":
      notice(st, key, `${a.command} was rejected: ${a.message}`, "err");
      return;
    default:
      // Includes input_acknowledged (a developer detail) and every event kind this build does not know.
      return;
  }
}

/** The agent compacted its context: how much it dropped, and when. */
export function compactionWords(c: CompactionInfo, at: number): string {
  return `History compacted at ${clockTime(at)} (${c.trigger}): ${compactNumber(c.pre_tokens)} → ${compactNumber(c.post_tokens)} tokens in ${(c.duration_ms / 1000).toFixed(1)}s · ${compactNumber(c.cumulative_dropped_tokens)} dropped so far`;
}

/** The provider's rate-limit status, and when it resets. */
export function rateLimitWords(r: RateLimitInfo): string {
  return `Rate limit: ${r.status} (${r.rate_limit_type}) · resets ${clockTime(r.resets_at * 1000)}${r.is_using_overage ? " · using overage" : ""}`;
}

/** Why a command was dropped, as a clause after "not sent" ("interrupted", "the session ended" ...). */
export function dropWords(reason: NotSentCause): string {
  switch (reason) {
    case "interrupted":
      return "interrupted";
    case "shutdown":
    case "process_ended":
      return "session ended";
    case "unencodable":
      return "the agent could not take it";
    case "refused":
      return "the agent refused it";
    case "unsupported":
      return "the agent does not support it";
    case "rejected":
      return "rejected";
    case "not_running":
      return "the session was not running";
    case "withdrawn":
      return "no longer needed";
    case "write_failed":
      return "writing to the agent failed";
  }
  return String(reason);
}

/** The line shown under a message that did not reach the agent; `null` for a normal one. */
export function deliveryNote(d: Delivery): { text: string; detail?: string } | null {
  if (d.state !== "not_sent") return null;
  return { text: `Not sent — ${dropWords(d.cause)}`, detail: d.detail };
}

/** Same item content: the same fields, equal by identity (nested records and lists by their members). */
function sameItem(a: Item, b: Item): boolean {
  const ka = Object.keys(a);
  if (ka.length !== Object.keys(b).length) return false;
  return ka.every((k) => {
    const x = (a as Record<string, unknown>)[k];
    const y = (b as Record<string, unknown>)[k];
    if (Object.is(x, y)) return true;
    if (Array.isArray(x) && Array.isArray(y)) return x.length === y.length && x.every((v, i) => Object.is(v, y[i]));
    if (k === "delivery") return sameDelivery(x as Delivery, y as Delivery) || JSON.stringify(x) === JSON.stringify(y);
    return false;
  });
}

export interface TranscriptFold {
  /**
   * The items of `events` (oldest first, contiguous). The result depends only on `events`, never on
   * earlier calls; earlier calls only make it cheap:
   * - the same array again returns the same items;
   * - an array that extends the previous one (live events) folds only the new events;
   * - anything else (older events prepended, a different window) folds from the start, then hands
   *   back the previous object for every item that came out unchanged.
   * An item that is not affected by the new events is the very same object as before.
   */
  update(events: readonly SessionEvent[]): readonly Item[];
}

export function createTranscriptFold(): TranscriptFold {
  let st = blank();
  let seen: readonly SessionEvent[] = [];
  let out: readonly Item[] = [];
  return {
    update(events) {
      if (events === seen) return out;
      const n = st.count;
      const extended = events.length >= n && (n === 0 || (events[0] === seen[0] && events[n - 1] === seen[n - 1]));
      let from = n;
      let before: readonly Item[] | null = null;
      if (!extended) {
        before = out;
        st = blank();
        from = 0;
      }
      for (let i = from; i < events.length; i++) apply(st, events[i]);
      st.count = events.length;
      seen = events;
      if (before) {
        const old = new Map(before.map((it) => [it.key, it]));
        st.items.forEach((it, i) => {
          const prev = old.get(it.key);
          if (prev && sameItem(prev, it)) st.items[i] = prev;
        });
        st.dirty = st.items.length !== before.length || st.items.some((it, i) => it !== before![i]);
      }
      if (st.dirty) {
        out = st.items.slice();
        st.dirty = false;
      }
      return out;
    },
  };
}

/** Fold wire events into renderable transcript items (deltas merge, tool calls join by call_id). */
export function buildItems(events: readonly SessionEvent[]): readonly Item[] {
  return createTranscriptFold().update(events);
}

/** Short one-line summary of a tool's input (file, command, pattern ...). */
export function toolArg(input: unknown): string {
  if (!input || typeof input !== "object") return input == null ? "" : String(input);
  const o = input as Record<string, unknown>;
  for (const k of ["command", "file_path", "path", "pattern", "question", "instruction", "description", "url", "query"]) {
    if (typeof o[k] === "string") return k === "pattern" && typeof o.path === "string" ? `"${o[k]}" in ${o.path}` : (o[k] as string);
  }
  return JSON.stringify(input).slice(0, 120);
}

const MEDIA_BLOCKS = new Set(["image", "audio", "resource", "resource_link", "document"]);

export function resultText(r: unknown): string {
  if (r == null) return "";
  if (typeof r === "string") return r;
  // ACP tool content: [{type: "content", content: {type: "text", text}}, …] and claude content
  // blocks: show the text; images and files are shown as parts.
  if (Array.isArray(r)) {
    const texts: string[] = [];
    let other = false;
    for (const c of r) {
      const inner = ((c as { content?: unknown })?.content ?? c) as { text?: unknown; type?: unknown } | null;
      if (typeof inner?.text === "string") texts.push(inner.text);
      else if (!MEDIA_BLOCKS.has(String(inner?.type))) other = true;
    }
    if (!other) return texts.join("\n");
  }
  return JSON.stringify(r, null, 2);
}
