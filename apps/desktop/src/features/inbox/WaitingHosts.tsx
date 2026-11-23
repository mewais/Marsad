import { hostPath, Link } from "../../router";
import { fleetLive, useAppState, useHosts, useWaitingHosts } from "../../state";
import { Icon } from "../../ui";
import { SignInButton } from "../hosts/SignInButton";

const DISCONNECTED = new Set(["reconnecting", "auth_needed", "failed", "incompatible", "stopped"]);

/**
 * Compact one-line note at the top of the Inbox naming any disconnected/failed hosts, with a
 * one-click Sign in button for any held host and a link to the Hosts page for details. Detailed
 * error text belongs on the Hosts page only.
 */
export function WaitingHosts() {
  const live = useAppState(fleetLive);
  const hosts = useHosts();
  const waiting = useWaitingHosts();
  if (!live) return null;
  const down = hosts.filter((h) => h.link.link === "remote" && DISCONNECTED.has(h.link.state.state));
  if (!down.length) return null;
  const names = down.map((h) => h.name).join(", ");
  const to = down.length === 1 ? hostPath(down[0].id) : "/hosts";
  return (
    <div className="hw" role="status" aria-label="Disconnected hosts" data-testid="waiting-hosts">
      <Icon name="warn" size="sm" className="c-needs" />
      <span className="hw-text" title={names}>
        Disconnected: <b>{names}</b>
      </span>
      {waiting.length ? (
        <span className="hw-acts">
          {waiting.length === 1 ? (
            <SignInButton host={waiting[0]} />
          ) : (
            waiting.map((h) => <SignInButton key={h.id} host={h} label={`Sign in to ${h.name}`} />)
          )}
        </span>
      ) : null}
      <Link to={to} className="hw-link">Details on Hosts</Link>
    </div>
  );
}
