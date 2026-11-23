import { useNavigate } from "../../router";
import { droppedWords, keepRunningWords, useWaitingHosts } from "../../state";
import { Button, Icon } from "../../ui";
import { SignInButton } from "../hosts/SignInButton";

/** The most hosts listed at the top of the Inbox; the rest are one click away on the Hosts page. */
const SHOWN = 3;

/**
 * Hosts that wait for the user to sign in, above the sessions: compact rows, each with the one click
 * that starts the sign-in. Mirrors the hub's `held` links; nothing here is decided by the app.
 */
export function WaitingHosts() {
  const hosts = useWaitingHosts();
  const nav = useNavigate();
  if (!hosts.length) return null;
  return (
    <section className="hw" aria-label="Hosts waiting for you" data-testid="waiting-hosts">
      <div className="grp c-needs" role="presentation">
        <Icon name="key" size="sm" />
        Hosts waiting for you
        <span className="cnt">{hosts.length}</span>
        <span className="gline" />
        <span className="gact">Marsad will not reconnect these on its own: signing in needs you</span>
      </div>
      {hosts.slice(0, SHOWN).map((h) => (
        <div key={h.id} className="hw-row" data-host={h.id}>
          <span className="hw-n">{h.name}</span>
          <span className="hw-w">{droppedWords(h.name, h.cause)} {keepRunningWords(h.sessions)}</span>
          <SignInButton host={h} />
        </div>
      ))}
      {hosts.length > SHOWN ? (
        <div className="hw-row"><span className="faint small">and {hosts.length - SHOWN} more</span><span className="sp" /><Button size="sm" variant="ghost" onClick={() => nav("/hosts")}>Open Hosts <Icon name="chevr" size="xs" /></Button></div>
      ) : null}
    </section>
  );
}
