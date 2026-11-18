import { useEffect, useState } from "react";
import { errorMessage } from "../../bridge/api";
import type { DriverInventory, HostId } from "../../bridge/wire";
import { useApi, useAppState } from "../../state";

/**
 * The host's inventory for one project folder: models, modes and the CLI's own resumable sessions
 * for that folder. Falls back to the host's live inventory (no sessions) while loading.
 */
export function useInventory(host: HostId, project: string, enabled = true): { drivers: DriverInventory[]; loading: boolean; error: string } {
  const api = useApi();
  const live = useAppState((s) => s.inventory[host]);
  const [r, setR] = useState<{ key: string; drivers: DriverInventory[]; error: string } | null>(null);
  const key = `${host}|${project}`;
  // A project's probes run after the first reply (`refreshing`); ask again whenever the host's
  // inventory feed moves until they have finished. (The feed bumps on every discovery change.)
  const refreshing = r?.key === key && r.drivers.some((d) => d.refreshing);
  const tick = refreshing ? live : undefined;
  useEffect(() => {
    if (!enabled || !host) return;
    let off = false;
    const h = setTimeout(() => {
      api.inventory(host, project.trim() || null).then(
        (x) => !off && setR({ key, drivers: x.drivers, error: "" }),
        (e) => !off && setR({ key, drivers: [], error: errorMessage(e) }),
      );
    }, r?.key === key ? 0 : 500);
    return () => {
      off = true;
      clearTimeout(h);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, host, project, key, enabled, tick]);
  const fresh = r?.key === key;
  // A folder's entry starts out unresolved (`installed: false` while its probes run): until
  // then the host-wide entry answers whether the CLI is installed and what it offers.
  const drivers = fresh && r.drivers.length
    ? r.drivers.map((d) => {
        const host = d.refreshing && !d.installed ? live?.find((l) => l.driver === d.driver) : undefined;
        return host ? { ...host, sessions: d.sessions, refreshing: true } : d;
      })
    : live ?? [];
  return { drivers, loading: !fresh, error: fresh ? r.error : "" };
}
