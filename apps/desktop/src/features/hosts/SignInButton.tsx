import type { HostId } from "../../bridge/wire";
import { reconnectPendingKey, useActions, usePending, useReach } from "../../state";
import { Button } from "../../ui";

/**
 * The one click that starts a sign-in on a host that waits for the user (`hosts.reconnect`). Nothing
 * else ever calls it for such a host: a click can send a push to the user's phone or use up a
 * one-time code. Pending while the hub takes the request; off, with the reason, while the app has no
 * link to the hub; the hub's refusal is shown by the action.
 */
export function SignInButton({ host, label = "Sign in", variant = "primary" }: { host: { id: HostId; name: string }; label?: string; variant?: "primary" | "default" }) {
  const actions = useActions();
  const busy = !!usePending(reconnectPendingKey(host.id));
  const reach = useReach();
  return (
    <Button size="sm" variant={variant} icon="key" pending={busy} blocked={reach} title={label} onClick={() => void actions.reconnectHost(host.id, host.name)}>
      <span className="ell">{label}</span>
    </Button>
  );
}
