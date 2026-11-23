import type { Api } from "../../bridge/api";
import type { Attachment, LaunchReply, LaunchRequest } from "../../bridge/wire";
import { charCount, sumBytes, type Trace } from "../../state/diaglog";
import { upload, type PendingFile } from "../session/attachments";

export type Step = { step: "creating" } | { step: "uploading"; index: number; total: number; name: string; done: number; size: number } | { step: "sending" };

export type LaunchOutcome =
  | { ok: true; reply: LaunchReply }
  /** The session was not created: nothing to clean up, the dialog stays as it is. */
  | { ok: false; stage: "create"; error: unknown }
  /** The session exists but the attachments or the first prompt did not go through. */
  | { ok: false; stage: "upload" | "send"; reply: LaunchReply; error: unknown };

/**
 * Launch a session whose first prompt carries attachments. The session does not exist yet, so:
 * create it (no first prompt), upload each file to its attachments folder over the link, then
 * send the first prompt with them. Progress is reported per step. A failure after the session
 * was created is reported with its reply, so the caller can open it with the draft intact.
 *
 * `trace`: the user action this is (one trace id for the launch and the prompt, and the `ui.*`
 * events that say what the app did); without one nothing is traced.
 */
export async function launchWithAttachments(api: Api, request: LaunchRequest, text: string, files: PendingFile[], onStep: (s: Step) => void, trace?: Trace): Promise<LaunchOutcome> {
  onStep({ step: "creating" });
  let reply: LaunchReply;
  try {
    trace?.launch(request.host ?? "", request, true, files.length);
    reply = await api.launch({ ...request, first_prompt: null }, trace?.id);
  } catch (error) {
    return { ok: false, stage: "create", error };
  }
  const uploadAll = async () => {
    const done: Attachment[] = [];
    for (const [i, f] of files.entries()) {
      onStep({ step: "uploading", index: i + 1, total: files.length, name: f.name, done: 0, size: f.size });
      done.push(await upload(api, reply.host, reply.id, f, (n) => onStep({ step: "uploading", index: i + 1, total: files.length, name: f.name, done: n, size: f.size })));
    }
    return done;
  };
  let attachments: Attachment[];
  try {
    attachments = trace ? await trace.upload({ count: files.length, bytes: sumBytes(files) }, uploadAll) : await uploadAll();
  } catch (error) {
    return { ok: false, stage: "upload", reply, error };
  }
  try {
    onStep({ step: "sending" });
    trace?.prompt(reply.host, reply.id, "prompt", charCount(text), { count: attachments.length, bytes: sumBytes(attachments) });
    await api.command(reply.host, reply.id, { command: "prompt", text, attachments }, trace?.id);
  } catch (error) {
    return { ok: false, stage: "send", reply, error };
  }
  return { ok: true, reply };
}
