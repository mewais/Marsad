import type { SessionSnapshot } from "../../bridge/wire";
import { sessionPath, useNavigate } from "../../router";
import { compactPendingKey, forkPendingKey, useActions, usePending } from "../../state";
import { Button } from "../../ui";

/**
 * Compact is offered only when the CLI both supports the command and lists it: the menu the CLI reported is the truth,
 * not a guess from the driver's name.
 */
export function offersCompact(snap: Pick<SessionSnapshot, "capabilities" | "slash_commands">): boolean {
  return snap.capabilities.compact && snap.slash_commands.some((c) => c.name === "compact");
}

/** Compacting runs as a turn of its own, so it waits until the agent is idle (starting, working or waiting for the user all count as busy). */
export const isBusy = (snap: Pick<SessionSnapshot, "phase">): boolean => snap.phase.state !== "idle";

/**
 * Fork copies a conversation that has something in it: the agent must be idle and have completed at least one turn.
 * The runtime refuses otherwise; the button is not offered, so the user never sees a refusal they cannot act on.
 */
export function offersFork(snap: Pick<SessionSnapshot, "capabilities" | "phase" | "turns_completed">): boolean {
  return snap.capabilities.fork && snap.phase.state === "idle" && snap.turns_completed >= 1;
}

/** The header's Compact action for a session whose process is running. Hidden when the CLI does not offer it. */
export function CompactButton({ sessionKey, snap, blocked }: { sessionKey: string; snap: SessionSnapshot; blocked: string | null }) {
  const actions = useActions();
  const pending = usePending(compactPendingKey(sessionKey)) === "pending";
  if (!offersCompact(snap)) return null;
  const busy = isBusy(snap);
  return (
    <Button size="sm" icon="sparkle" pending={pending} blocked={blocked} disabled={busy}
      title={busy ? "Wait until the agent is idle; it compacts between turns" : "Summarize the conversation so far to free up context"}
      onClick={() => void actions.compact(sessionKey)}>
      Compact
    </Button>
  );
}

/**
 * The header's Fork action. On success the page moves to the new session (the parent is never changed). Hidden unless
 * the session is idle with a completed turn to copy.
 */
export function ForkButton({ host, sessionKey, snap, blocked }: { host: string; sessionKey: string; snap: SessionSnapshot; blocked: string | null }) {
  const actions = useActions();
  const nav = useNavigate();
  const pending = usePending(forkPendingKey(sessionKey)) === "pending";
  if (!offersFork(snap)) return null;
  const fork = async () => {
    const made = await actions.fork(sessionKey);
    if (made) nav(sessionPath(host, made.id));
  };
  return (
    <Button size="sm" variant="ghost" icon="branch" pending={pending} blocked={blocked} title="Start a new session with a copy of this conversation; this session stays as it is"
      onClick={() => void fork()}>
      Fork
    </Button>
  );
}
