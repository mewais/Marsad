import type { Attachment, ContentPart, SessionEvent } from "../../bridge/wire";

export type Item =
  | { kind: "user"; key: string; text: string; queued: boolean; attachments: Attachment[] }
  | { kind: "assistant"; key: string; text: string }
  /** Non-text content: an image or file the agent showed, or one the CLI reported in a user message. */
  | { kind: "media"; key: string; role: "assistant" | "user"; parts: ContentPart[] }
  | { kind: "thinking"; key: string; text: string }
  | { kind: "tool"; key: string; callId: string; name: string; input: unknown; subagent: boolean; done: boolean; ok?: boolean; result?: unknown; status?: string; parts: ContentPart[] }
  | { kind: "notice"; key: string; text: string; tone: "info" | "err" };

/** Fold wire events into renderable transcript items (deltas merge, tool calls join by call_id). */
export function buildItems(events: SessionEvent[]): Item[] {
  const out: Item[] = [];
  const tools = new Map<string, number>();
  let lastMsg: string | undefined;
  for (const e of events) {
    const key = String(e.cursor);
    if (e.kind === "input") {
      const c = e.command;
      if (c.command === "prompt") out.push({ kind: "user", key, text: c.text, queued: e.status.status === "queued", attachments: c.attachments ?? [] });
      else if (c.command === "run_slash") out.push({ kind: "user", key, text: `/${c.name} ${c.args}`.trim(), queued: false, attachments: [] });
      lastMsg = undefined;
    } else if (e.kind === "agent") {
      const a = e.event;
      switch (a.type) {
        case "assistant_delta":
        case "thinking_delta": {
          const kind = a.type === "assistant_delta" ? "assistant" : "thinking";
          const prev = out[out.length - 1];
          if (prev && prev.kind === kind && (a.message_id === undefined || a.message_id === lastMsg)) prev.text += a.text;
          else out.push({ kind, key, text: a.text });
          lastMsg = a.message_id ?? undefined;
          break;
        }
        case "tool_call_started":
          tools.set(a.call_id, out.length);
          out.push({ kind: "tool", key, callId: a.call_id, name: a.name, input: a.input, subagent: a.is_subagent, done: false, parts: [] });
          lastMsg = undefined;
          break;
        case "tool_call_updated": {
          const t = out[tools.get(a.call_id) ?? -1];
          if (t?.kind === "tool") {
            t.status = a.status ?? undefined;
            // ACP agents send the arguments after announcing the call.
            if (a.input != null) t.input = a.input;
            if (a.parts?.length) t.parts = [...t.parts, ...a.parts];
          }
          break;
        }
        case "assistant_content":
        case "user_content": {
          const role = a.type === "assistant_content" ? "assistant" : "user";
          const prev = out[out.length - 1];
          if (prev?.kind === "media" && prev.role === role) prev.parts = [...prev.parts, ...a.parts];
          else out.push({ kind: "media", key, role, parts: a.parts });
          lastMsg = undefined;
          break;
        }
        case "tool_call_finished": {
          const t = out[tools.get(a.call_id) ?? -1];
          if (t?.kind === "tool") Object.assign(t, { done: true, ok: a.ok, result: a.result, parts: a.parts?.length ? a.parts : t.parts });
          break;
        }
        case "notice":
          out.push({ kind: "notice", key, text: a.message, tone: "info" });
          break;
        case "command_rejected":
          out.push({ kind: "notice", key, text: `${a.command} was rejected: ${a.message}`, tone: "err" });
          break;
        default:
          break;
      }
    } else if (e.kind === "notice") out.push({ kind: "notice", key, text: e.message, tone: "info" });
    else if (e.kind === "lost") out.push({ kind: "notice", key, text: `Session lost: ${e.reason}`, tone: "err" });
    else if (e.kind === "phase" && e.phase.state === "failed") out.push({ kind: "notice", key, text: `Failed: ${e.phase.reason}`, tone: "err" });
  }
  return out;
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
