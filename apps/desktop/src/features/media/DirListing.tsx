import { useEffect, useState } from "react";
import { errorMessage } from "../../bridge/api";
import type { FsListing } from "../../bridge/wire";
import { useApi } from "../../state";
import { Icon } from "../../ui";
import { baseName } from "./fetch";
import { openStat } from "./PathChip";
import type { PreviewRequest } from "./preview";

/**
 * A folder opened from a chip: a plain listing (the host's `fs.list`). Nothing in it is read
 * until you click an entry; a file goes through the same open flow as any chip (a grant for
 * exactly that file when it lies outside the session's folder, a confirm for secret places).
 */
export function DirListing({ req }: { req: PreviewRequest }) {
  const api = useApi();
  const start = "path" in req.target ? req.target.path : "";
  const [path, setPath] = useState(start);
  const [list, setList] = useState<FsListing | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    let off = false;
    setError("");
    api.fs.list(req.host, { path, show_hidden: true, dirs_only: false }).then(
      (l) => !off && setList(l),
      (e) => !off && setError(errorMessage(e)),
    );
    return () => {
      off = true;
    };
  }, [api, req.host, path]);
  const open = async (p: string) => {
    const r = await api.media.stat(req.host, req.session, [p], true).catch(() => null);
    const f = r?.files[0];
    if (f) await openStat(api, req, f);
  };
  return (
    <div className="dirb" data-testid="dir-listing">
      <div className="dirb-h">
        <button type="button" className="iconbtn" aria-label="Parent folder" disabled={!list?.parent} onClick={() => list?.parent && setPath(list.parent)}><Icon name="arrowdown" size="sm" className="rot180" /></button>
        <span className="mono dirb-path" title={list?.path}>{list?.path ?? path}</span>
      </div>
      {error ? <div className="c-failed dirb-err" role="alert">{error}</div> : null}
      <ul className="dirb-l" aria-label="Entries">
        {!list && !error ? <li className="faint">Loading…</li> : list && list.entries.length === 0 ? <li className="faint">Empty folder</li> : null}
        {list?.entries.map((e) => (
          <li key={e.path}>
            <button type="button" onClick={() => (e.is_dir ? setPath(e.path) : void open(e.path))}>
              <Icon name={e.is_dir ? "folder" : "file"} size="sm" />
              <span>{e.name}</span>
            </button>
          </li>
        ))}
      </ul>
      {list?.truncated ? <div className="faint fs12">Showing the first 5000 entries</div> : null}
      <span className="sr-only">{baseName(path)}</span>
    </div>
  );
}
