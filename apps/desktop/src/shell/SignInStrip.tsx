import { SignInButton } from "../features/hosts/SignInButton";
import { useRoute } from "../router";
import { droppedWords, keepRunningWords, NO_RETRY_WORDS, useWaitingHosts } from "../state";
import { Icon } from "../ui";

/**
 * One strip, from any page: which hosts wait for the user to sign in, why Marsad is not reconnecting
 * them, and the click that starts it. It mirrors the hub's `held` links and decides nothing. The Inbox
 * shows the same hosts itself, at the top of its list, and the Hosts page in its list and detail, so
 * the strip stays out of their way there.
 */
export function SignInStrip() {
  const hosts = useWaitingHosts();
  const route = useRoute();
  if (!hosts.length || route.name === "inbox" || route.name === "hosts") return null;
  const one = hosts.length === 1 ? hosts[0] : null;
  return (
    <section className="banner warn" aria-label="Hosts waiting for you" aria-live="polite" data-testid="signin-strip">
      <Icon name="key" size="sm" />
      <div className="bt">
        {one ? (
          <>
            <b>{one.name} needs you to sign in.</b> <span>{droppedWords(one.name, one.cause)} {NO_RETRY_WORDS} {keepRunningWords(one.sessions)}</span>
          </>
        ) : (
          <>
            <b>{hosts.length} hosts need you to sign in.</b> <span>{hosts.map((h) => h.name).join(", ")}. Marsad will not reconnect them on its own: signing in needs you. Their sessions keep running there and reattach once you sign in.</span>
          </>
        )}
      </div>
      {hosts.map((h) => <SignInButton key={h.id} host={h} label={one ? "Sign in" : `Sign in to ${h.name}`} />)}
    </section>
  );
}
