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
  useEffect(() => {
    if (!enabled || !host) return;
    let off = false;
    const h = setTimeout(() => {
      api.inventory(host, project.trim() || null).then(
        (x) => !off && setR({ key, drivers: x.drivers, error: "" }),
        (e) => !off && setR({ key, drivers: [], error: errorMessage(e) }),
      );
    }, 200);
    return () => {
      off = true;
      clearTimeout(h);
    };
  }, [api, host, project, key, enabled]);
  const fresh = r?.key === key;
  return { drivers: fresh && r.drivers.length ? r.drivers : live ?? [], loading: !fresh, error: fresh ? r.error : "" };
}
