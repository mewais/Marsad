import type { Api, PickedFile } from "../../bridge/api";
import type { Attachment, HostId, PromptInput, SessionId } from "../../bridge/wire";
import type { PromptFiles } from "../../state";
import { sumBytes } from "../../state/diaglog";
import { b64ToBytes, bytesToB64, formatBytes, mimeFromName } from "../media/fetch";

/** A file chosen for the next prompt, not yet uploaded. */
export interface PendingFile {
  key: string;
  name: string;
  size: number;
  mime: string;
  /** Object URL of an image, for its thumbnail (revoked on remove/send). */
  thumb?: string;
  read: (offset: number, length: number) => Promise<Uint8Array>;
}

let seq = 0;
const nextKey = () => `att${Date.now().toString(36)}${(seq++).toString(36)}`;

const isImage = (mime: string) => mime.startsWith("image/");

/** A file from paste / drop / a browser file input. */
export function fromFile(f: File): PendingFile {
  const mime = f.type || mimeFromName(f.name);
  return {
    key: nextKey(),
    name: f.name || "pasted",
    size: f.size,
    mime,
    thumb: isImage(mime) && typeof URL.createObjectURL === "function" ? URL.createObjectURL(f) : undefined,
    read: async (offset, length) => new Uint8Array(await f.slice(offset, offset + length).arrayBuffer()),
  };
}

/** A file the desktop shell's native picker returned (read through the shell, ranges). */
export function fromPicked(api: Api, p: PickedFile): PendingFile {
  return {
    key: nextKey(),
    name: p.name,
    size: p.size,
    mime: mimeFromName(p.name),
    read: async (offset, length) => b64ToBytes((await api.desktop.readPicked(p.path, offset, length)).data),
  };
}

export type Handling = { ok: true; how: "image" | "embedded" | "path"; note: string } | { ok: false; note: string };

/**
 * How a driver takes this file, from its descriptor (`prompt_input`): never by driver name.
 * The note is the chip's tooltip; a refusal explains why.
 */
export function handling(f: { name: string; size: number; mime: string }, input: PromptInput): Handling {
  const mb = (n: number) => `${Math.round(n / 1024 / 1024)} MB`;
  if (input.max_attachments === 0) return { ok: false, note: "This agent takes no attachments." };
  if (isImage(f.mime) && input.image_mimes.includes(f.mime)) {
    if (f.size > input.max_image_bytes) return { ok: false, note: `Larger than the ${mb(input.max_image_bytes)} this agent takes for an image.` };
    return { ok: true, how: "image", note: "Sent as an image. Whether the agent can see it depends on its model." };
  }
  if (input.files === "none") {
    return { ok: false, note: input.image_mimes.length ? `This agent takes ${input.image_mimes.map((m) => m.slice(6).toUpperCase()).join(", ")} images only.` : "This agent takes no attachments." };
  }
  if (f.size > input.max_file_bytes) return { ok: false, note: `Larger than the ${mb(input.max_file_bytes)} limit for files.` };
  const text = f.mime.startsWith("text/") || ["application/json", "application/xml", "image/svg+xml"].includes(f.mime);
  if (input.files === "embedded" && text && f.size <= 256 * 1024) return { ok: true, how: "embedded", note: "Embedded in your message." };
  if (input.files === "embedded") return { ok: true, how: "path", note: "Uploaded to the session's folder on its host and passed to the agent as a link to that copy." };
  return { ok: true, how: "path", note: "Uploaded to the session's folder on its host; your message names its path and the agent reads it with its own tools." };
}

/**
 * Re-validate what is attached against another driver's limits: the files it can take (in
 * order, up to its maximum) and an explanation for each one dropped.
 */
export function revalidate<T extends { name: string; size: number; mime: string }>(files: T[], input: PromptInput): { keep: T[]; dropped: string[] } {
  const keep: T[] = [];
  const dropped: string[] = [];
  for (const f of files) {
    const h = handling(f, input);
    if (!h.ok) dropped.push(`${f.name}: ${h.note}`);
    else if (keep.length >= input.max_attachments) dropped.push(`${f.name}: this agent takes at most ${input.max_attachments} attachments.`);
    else keep.push(f);
  }
  return { keep, dropped };
}

/** Why `files` cannot all be sent, or null. */
export function batchProblem(files: PendingFile[], input: PromptInput): string | null {
  if (files.length > input.max_attachments) return `At most ${input.max_attachments} attachments per message.`;
  const bad = files.find((f) => !handling(f, input).ok);
  if (bad) return `${bad.name}: ${(handling(bad, input) as { note: string }).note}`;
  const embedded = files.filter((f) => handling(f, input).ok && (handling(f, input) as { how: string }).how !== "path").reduce((a, f) => a + f.size, 0);
  if (input.embeds_bytes && embedded > 5.5 * 1024 * 1024) return `Images and embedded files add up to ${formatBytes(embedded)}; one message takes 5.5 MB.`;
  return null;
}

/** Upload chunk size (decoded); the host takes at most 1 MiB. */
const CHUNK = 768 * 1024;

const uploadId = () => Array.from({ length: 16 }, () => "abcdefghijklmnopqrstuvwxyz0123456789"[Math.floor(Math.random() * 36)]).join("");

/** Upload one file to the session's host over the control link, in order, in chunks. */
export async function upload(api: Api, host: HostId, session: SessionId, f: PendingFile, onProgress?: (done: number) => void): Promise<Attachment> {
  const id = uploadId();
  let offset = 0;
  for (;;) {
    const bytes = await f.read(offset, CHUNK);
    const last = offset + bytes.length >= f.size;
    const r = await api.media.put(host, { id: session, upload: id, name: f.name, size: f.size, offset, data: bytesToB64(bytes), last });
    offset = r.received;
    onProgress?.(offset);
    if (r.attachment) return r.attachment;
    if (last || bytes.length === 0) throw new Error(`the upload of ${f.name} did not finish`);
  }
}

/** What `actions.sendPrompt` needs of pending files: how many and how big (for the log), and the upload to the session's host. */
export function promptFiles(api: Api, host: HostId, session: SessionId, files: PendingFile[]): PromptFiles {
  return {
    count: files.length,
    bytes: sumBytes(files),
    upload: async () => {
      const done: Attachment[] = [];
      for (const f of files) done.push(await upload(api, host, session, f));
      return done;
    },
  };
}
