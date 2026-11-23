import type { EndCode } from "../bridge/wire";

/**
 * How a request that waited for a human ended, in words. One vocabulary: Wasl owns the codes
 * (`EndCode`: how a spawn request or a handoff gate ended); the runtime adds three of its own
 * to a session's gate resolutions (`process_ended`, `session_stopped`, and the legacy
 * `withdrawn` of journals written before a resolution could say why). The words belong to the
 * app; the codes are the daemon's. A code this version does not know is said as such, with the
 * code, never guessed at.
 */
const WORDS: Record<EndCode | "process_ended" | "session_stopped" | "withdrawn", string> = {
  launched: "It was started.",
  denied: "It was denied.",
  turn_ended: "The agent’s turn ended before anyone answered, so the request could no longer be answered.",
  turn_interrupted: "The turn that asked was interrupted before the request was answered.",
  cancelled: "It was cancelled.",
  caller_gone: "The agent’s tool call ended, so nothing was waiting for an answer any more.",
  session_ended: "The asking session ended before the request was answered.",
  session_gone: "The asking session was deleted or could not be followed.",
  gate_gone: "The request was taken down by something other than the agent that raised it.",
  deadline_passed: "No answer came before its deadline.",
  task_ended: "It was approved, but the task it was for had ended in the meantime.",
  refused: "It was refused.",
  launch_failed: "It was approved, but the agent could not be started.",
  gate_unavailable: "The request could not be put to you on the asking session.",
  decided_elsewhere: "It was decided elsewhere, without this request.",
  unrecorded: "How it ended was not recorded.",
  process_ended: "The session’s process ended while it was waiting.",
  session_stopped: "The session was ended while it was waiting.",
  withdrawn: "The request was taken down by its owner; no reason was given.",
};

/** A short sentence for an ending code (a spawn outcome's `end.code`, or a gate resolution's `reason.code`). */
export function endWords(code: string): string {
  return Object.prototype.hasOwnProperty.call(WORDS, code) ? WORDS[code as keyof typeof WORDS] : `It ended for a reason this version does not know (${code}).`;
}

/** "<What> ended without your answer. <why in words> <what the resolver said>": the one sentence for a request that went away (`what`: "the request to start scout"). */
export function endedSentence(what: string, why: { code: string; detail?: string | null }): string {
  return `${what.charAt(0).toUpperCase()}${what.slice(1)} ended without your answer. ${endWords(why.code)}${why.detail ? ` ${why.detail}` : ""}`;
}
