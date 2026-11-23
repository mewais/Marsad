import type { AuthPrompt, HostId, LinkState, SignInRepeat } from "../bridge/wire";

/**
 * Everything the app knows about hosts that need the user at sign-in, and nothing it decides: which
 * hosts wait for the user is the daemon's flag (`held` on the link), what is asked of them is the
 * daemon's prompt. Here that is only read, ordered and worded.
 */

/** A remote link the hub is deliberately not reconnecting: it needs the user at sign-in (`auth_needed` with `held`). */
export type HeldLink = Extract<LinkState, { state: "auth_needed" }> & { held: true };
export function heldLink(l: LinkState): HeldLink | null {
  return l.state === "auth_needed" && l.held === true ? (l as HeldLink) : null;
}

/** A host that waits for the user to sign in, as the strip, the Inbox and the badges show it. */
export interface WaitingHost {
  id: HostId;
  name: string;
  /** Why the connection dropped, in the hub's words; absent when the hub started without it connected. */
  cause?: string;
  /** The hub's own words for why it waits. */
  detail: string;
  /** How many sessions of the host are listed (as last known). */
  sessions: number;
}

export const sameWaitingHosts = (a: WaitingHost[], b: WaitingHost[]) =>
  a.length === b.length && a.every((x, i) => x.id === b[i].id && x.name === b[i].name && x.cause === b[i].cause && x.detail === b[i].detail && x.sessions === b[i].sessions);

/** One sentence for why a prompt is a second approval within the same sign-in. An unknown reason still says it is a repeat. */
export function signInRepeatWords(r: SignInRepeat | string | null | undefined): string | null {
  switch (r) {
    case undefined:
    case null:
      return null;
    case "helper_restart":
      return "The host asks again because Marsad had to reconnect after updating its helper there.";
    case "id_restart":
      return "The host asks again because Marsad had to reconnect after the host answered under a different identity.";
    case "windows_shell":
      return "The host asks again because Marsad had to reconnect after switching to the shell Windows hosts need.";
    default:
      return "The host asks again.";
  }
}

/** Why a host that waits for the user is not connected, from what the hub reports. */
export function droppedWords(name: string, cause?: string): string {
  return cause ? `The connection to ${name} dropped: ${cause.replace(/[.\s]+$/, "")}.` : `${name} is not connected.`;
}

/** The hub does not reconnect a held host by itself; saying why is part of saying it. */
export const NO_RETRY_WORDS = "Marsad will not reconnect it on its own: signing in needs you.";

/** A host's sessions stay listed as last known; the daemon keeps them running and reattaches them after the link is back. */
export function keepRunningWords(sessions: number): string {
  return sessions === 0 ? "" : sessions === 1 ? "Its session keeps running there and reattaches once you sign in." : `Its ${sessions} sessions keep running there and reattach once you sign in.`;
}

/** What a host asks of the user, in a few words. */
export function askWords(kind: AuthPrompt["kind"]): string {
  return kind === "secret" ? "a password" : kind === "code" ? "a one-time code" : kind === "confirm" ? "a yes or no" : "approval";
}

/** A sign-in question and the host it is for. */
export interface SignInQuestion {
  prompt: AuthPrompt;
  hostName: string;
}

export const sameSignInQueue = (a: SignInQuestion[], b: SignInQuestion[]) =>
  a.length === b.length && a.every((x, i) => x.prompt === b[i].prompt && x.hostName === b[i].hostName);

/**
 * What the sign-in dialog shows. `asking`: one question, bound to its id (an answer typed for it is
 * sent with that id, so it can only ever reach that question). `withdrawn`: the question on screen
 * went away without the user's act (its attempt ended); the dialog says so instead of putting
 * another host's question under the user's cursor.
 *
 * `answerable`: the question takes something from the user (a password, a code, a yes or no). An
 * `info` question is a notice (approve the push on your phone, touch your key): it ends by itself
 * when the user does that, so its disappearance is the expected outcome, not a withdrawal.
 */
export type SignInScreen =
  | { kind: "none" }
  | { kind: "asking"; host: HostId; hostName: string; id: number; answerable: boolean }
  | { kind: "withdrawn"; host: HostId; hostName: string; id: number };

export const NO_SCREEN: SignInScreen = { kind: "none" };

/**
 * The next screen given the queue (pure; the same object when nothing changed). `settled`: ids the
 * user answered or cancelled, whose disappearance is the expected result and moves on silently.
 * A `withdrawn` screen stays until the user acknowledges it.
 */
export function reconcileSignInScreen(screen: SignInScreen, queue: SignInQuestion[], settled: ReadonlySet<number>): SignInScreen {
  if (screen.kind === "withdrawn") return screen;
  if (screen.kind === "asking") {
    if (queue.some((q) => q.prompt.id === screen.id && q.prompt.host === screen.host)) return screen;
    if (screen.answerable && !settled.has(screen.id)) return { kind: "withdrawn", host: screen.host, hostName: screen.hostName, id: screen.id };
  }
  const first = queue[0];
  return first ? { kind: "asking", host: first.prompt.host, hostName: first.hostName, id: first.prompt.id, answerable: first.prompt.kind !== "info" } : NO_SCREEN;
}

/** The daemon refused an answer or a cancel because the question is not pending any more (its attempt ended, or it was answered). */
export function isPromptGone(e: unknown): boolean {
  const r = e as { code?: string; data?: { error?: string } | null } | undefined;
  return r?.code === "not_found" || r?.data?.error === "not_found";
}
