import type { FileStat } from "../../bridge/wire";
import { errorMessage } from "../../bridge/api";
import { useApi } from "../../state";
import { Icon } from "../../ui";
import { toast } from "../../ui/toast";
import type { Api } from "../../bridge/api";
import { baseName, formatBytes } from "./fetch";
import { useMediaScope, type MediaScope } from "./Parts";
import { isSecretPath } from "./pathRefs";
import { confirmSecret, openPreview } from "./preview";

/**
 * Open what a path chip names. A well-known secret location is confirmed once first. A file
 * outside the session's folder is read only through a grant for exactly that file, asked for
 * now (the daemon's automatic read roots never change); a folder opens as a listing.
 */
export async function openStat(api: Api, scope: MediaScope, f: FileStat, line?: number | null): Promise<void> {
  const path = f.resolved;
  if (isSecretPath(path) && !(await confirmSecret(path))) return;
  const name = baseName(f.path.replace(/[\\/]+$/, "") || f.path);
  try {
    if (f.is_dir) return openPreview({ ...scope, target: { path }, name, mime: f.mime, size: null, dir: true });
    const target = f.outside ? { path, outside: true, grant: (await api.media.grant(scope.host, { id: scope.session, path })).token } : { path };
    openPreview({ ...scope, target, name, mime: f.mime, size: f.size, line: line ?? null });
  } catch (e) {
    toast({ tone: "err", title: "Could not open the file", detail: errorMessage(e) });
  }
}

/** A chip for a path that exists on the session's host (file or folder). */
export function PathChip({ stat, line }: { stat: FileStat; line?: number | null }) {
  const api = useApi();
  const scope = useMediaScope();
  const label = `${baseName(stat.path.replace(/[\\/]+$/, "") || stat.path)}${line ? `:${line}` : ""}`;
  const secret = isSecretPath(stat.resolved);
  return (
    <span className="fchip" data-testid="file-ref" data-outside={stat.outside ? "1" : undefined} title={`${stat.resolved}${stat.is_dir ? "" : ` · ${formatBytes(stat.size)}`}${stat.outside ? "\nOutside the session's folder: opens only when you click" : ""}`}>
      <button type="button" className="plainbtn fchip-b" disabled={!scope} onClick={() => scope && void openStat(api, scope, stat, line)}>
        <Icon name={stat.is_dir ? "folder" : secret ? "lock" : stat.mime.startsWith("image/") ? "eye" : "file"} size="xs" />
        <span className="fchip-n">{label}{stat.is_dir ? "/" : ""}</span>
        {stat.is_dir ? null : <span className="faint fchip-s">{formatBytes(stat.size)}</span>}
      </button>
    </span>
  );
}
