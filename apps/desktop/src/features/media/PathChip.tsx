import type { FileStat } from "../../bridge/wire";
import { Icon } from "../../ui";
import { baseName, formatBytes } from "./fetch";
import { useMediaScope } from "./Parts";
import { openPreview } from "./preview";

/** A chip for a path that exists inside the session's folder; a `path:line` mention opens at that line. */
export function PathChip({ stat, line }: { stat: FileStat; line?: number | null }) {
  const scope = useMediaScope();
  const label = `${baseName(stat.path)}${line ? `:${line}` : ""}`;
  return (
    <span className="fchip" data-testid="file-ref" title={`${stat.resolved} · ${formatBytes(stat.size)}`}>
      <button type="button" className="plainbtn fchip-b" disabled={!scope} onClick={() => scope && openPreview({ ...scope, target: { path: stat.resolved }, name: baseName(stat.path), mime: stat.mime, size: stat.size, line: line ?? null })}>
        <Icon name={stat.mime.startsWith("image/") ? "eye" : "file"} size="xs" />
        <span className="fchip-n">{label}</span>
        <span className="faint fchip-s">{formatBytes(stat.size)}</span>
      </button>
    </span>
  );
}
