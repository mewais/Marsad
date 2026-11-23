import { useEffect, useState } from "react";
import { errorMessage } from "../../bridge/api";
import type { FsListing, HostId, RecentProject } from "../../bridge/wire";
import { useApi, useHost } from "../../state";
import { toast } from "../../ui/toast";
import { Button, Icon, Popover } from "../../ui";

/** Project folder: free text, recent projects on this host, and a directory browser over `fs.list`. */
export function ProjectField({ host, value, onChange, recent, error }: { host: HostId; value: string; onChange: (v: string) => void; recent: RecentProject[]; error?: string | null }) {
  const api = useApi();
  const hv = useHost(host);
  // A WSL host's files are visible to Windows: offer the native picker rooted in the distro
  // (it answers a Linux path; `/mnt/c/...` is shown as is in the browser).
  const native = hv?.distro && hv.localFiles ? hv.distro : null;
  const pickNative = () =>
    api.desktop.pickFolder({ title: "Choose a project folder", defaultPath: value || undefined, distro: native }).then(
      (r) => r.path && onChange(r.path),
      (e: unknown) => toast({ tone: "err", title: "Could not open the folder picker", detail: errorMessage(e) }),
    );
  const mine = recent.filter((r) => r.host === host && r.path !== value).slice(0, 4);
  return (
    <div className="field">
      <label htmlFor="ns-cwd">Project folder</label>
      <div className="input">
        <Icon name="folder" size="sm" />
        <input id="ns-cwd" value={value} onChange={(e) => onChange(e.target.value)} placeholder="~/src/project" spellCheck={false} aria-invalid={error ? true : undefined} aria-describedby={error ? "ns-cwd-e" : undefined} />
        {native ? <button type="button" className="linkbtn" onClick={() => void pickNative()}>Windows picker…</button> : null}
        <Popover align="right" className="dirpop" trigger={({ props }) => <button type="button" className="linkbtn" {...props}>Browse…</button>}>
          {({ close }) => <DirBrowser host={host} start={value || "~"} onPick={(p) => { onChange(p); close(); }} />}
        </Popover>
      </div>
      {error ? <span id="ns-cwd-e" className="err" role="alert" data-testid="ns-cwd-error">{error}</span> : null}
      {mine.length ? (
        <span className="hint recent-links">
          Recent:{" "}
          {mine.map((r, i) => (
            <span key={r.path}>
              {i ? " · " : ""}
              <button type="button" className="linkbtn" onClick={() => onChange(r.path)}>{r.path}</button>
            </span>
          ))}
        </span>
      ) : null}
    </div>
  );
}

export function DirBrowser({ host, start, onPick }: { host: HostId; start: string; onPick: (path: string) => void }) {
  const api = useApi();
  const [path, setPath] = useState(start);
  const [list, setList] = useState<FsListing | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    let off = false;
    setError("");
    api.fs.list(host, { path, show_hidden: false, dirs_only: true }).then(
      (l) => !off && setList(l),
      (e) => {
        if (off) return;
        setError(errorMessage(e));
        if (path !== "~") setPath("~");
      },
    );
    return () => {
      off = true;
    };
  }, [api, host, path]);
  return (
    <div className="dirb" role="group" aria-label="Choose a folder">
      <div className="dirb-h">
        <button type="button" className="iconbtn" aria-label="Parent folder" disabled={!list?.parent} onClick={() => list?.parent && setPath(list.parent)}><Icon name="arrowdown" size="sm" className="rot180" /></button>
        <span className="mono dirb-path" title={list?.path}>{list?.path ?? path}</span>
      </div>
      {error ? <div className="c-failed dirb-err" role="alert">{error}</div> : null}
      <ul className="dirb-l" aria-label="Folders">
        {!list ? <li className="faint">Loading…</li> : list.entries.length === 0 ? <li className="faint">No folders here</li> : null}
        {list?.entries.map((e) => (
          <li key={e.path}>
            <button type="button" onClick={() => setPath(e.path)} onDoubleClick={() => onPick(e.path)}>
              <Icon name={e.is_git_repo ? "branch" : "folder"} size="sm" />
              <span>{e.name}</span>
              {e.is_git_repo ? <span className="faint">git</span> : null}
            </button>
          </li>
        ))}
      </ul>
      <div className="dirb-f">
        <span className="faint">{list?.truncated ? "Showing the first 5000 entries" : ""}</span>
        <Button size="sm" variant="primary" disabled={!list} onClick={() => list && onPick(list.path)}>Use this folder</Button>
      </div>
    </div>
  );
}
