import { useActions, useAppState } from "../state";
import { Button, Icon, type IconName } from "../ui";

/** What a subscription is called to the user. */
function feedWords(name: string): string {
  if (name === "fleet") return "hosts and sessions";
  if (name === "workstreams") return "workstreams";
  if (name.startsWith("inventory:")) return "the agents installed on a host";
  if (name.startsWith("events:")) return "a session's transcript";
  return name;
}

type Problem = { tone: "info" | "warn" | "bad"; icon: IconName; spin?: boolean; title: string; detail: string; action?: { label: string; run: () => void } };

/**
 * What is wrong with the app's own connection, said once, above the page: the link to the local
 * daemon first (everything else depends on it), then live updates that stopped, then documents
 * that could not be loaded. Nothing here is a spinner for its own sake: each state says what the app
 * knows, why, and what it will do or what the user can do.
 */
export function ConnectionBanner() {
  const hub = useAppState((s) => s.hub);
  const feedErrors = useAppState((s) => s.feedErrors);
  const seen = useAppState((s) => s.loads.seen);
  const ready = useAppState((s) => s.fleetReady);
  const actions = useActions();
  const p = problem();
  if (!p) return null;
  return (
    <div className={`banner ${p.tone}`} role={p.tone === "bad" ? "alert" : "status"} data-testid="connection-banner" data-hub={hub.state}>
      <Icon name={p.icon} size="sm" spin={p.spin} />
      <div className="bt"><b>{p.title}</b> <span>{p.detail}</span></div>
      {p.action ? <Button size="sm" onClick={p.action.run}>{p.action.label}</Button> : null}
    </div>
  );

  function problem(): Problem | null {
    const kept = ready ? "What you see is the last known state, and actions that need the service are off until it is back." : "Nothing to show yet.";
    if (hub.state === "connecting") return { tone: "info", icon: "working", spin: true, title: "Connecting to the local service.", detail: `Starting it if it is not running. ${kept}` };
    if (hub.state === "reconnecting") return { tone: "warn", icon: "refresh", title: "Lost the local service; reconnecting.", detail: `${hub.error ? `${hub.error}. ` : ""}${kept}`, action: { label: "Try again now", run: () => void actions.reconnectHub() } };
    if (hub.state === "offline") return { tone: "bad", icon: "failed", title: "Cannot reach the local service.", detail: `${hub.error ? `${hub.error}. ` : ""}${kept} Marsad does not retry this on its own.`, action: { label: "Try again", run: () => void actions.reconnectHub() } };
    const ended = Object.entries(feedErrors);
    if (ended.length) {
      const what = [...new Set(ended.map(([n]) => feedWords(n)))].join(", ");
      return { tone: "bad", icon: "warn", title: `Live updates stopped for ${what}.`, detail: `${ended[0][1]}. What you see may be out of date.`, action: { label: "Try again", run: actions.retryLive } };
    }
    if (seen.state === "error") return { tone: "warn", icon: "warn", title: "Could not load which sessions you have seen.", detail: `${seen.error}. Until it loads, every finished session shows as not seen.`, action: { label: "Try again", run: () => actions.reload("seen") } };
    return null;
  }
}
