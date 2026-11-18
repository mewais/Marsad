import { useCallback, useEffect, useMemo, useState } from "react";
import { errorMessage } from "../../../bridge/api";
import type { GitDiff, GitFileChange, GitStatus } from "../../../bridge/wire";
import { splitKey } from "../../../bridge/wire";
import { useActions, useApi, useAppState } from "../../../state";
import { Button, ConfirmDialog, EmptyState, Icon, Segmented, Skeleton, toast } from "../../../ui";
import { DiffView } from "./DiffView";

const MAX_DIFF = 64 * 1024;

const letter = (f: GitFileChange) => {
  const k = f.unstaged ?? f.staged;
  return k === "added" || k === "untracked" ? "A" : k === "deleted" ? "D" : k === "renamed" ? "R" : k === "unmerged" ? "U" : "M";
};
const added = (f: GitFileChange) => f.staged_added + f.unstaged_added;
const removed = (f: GitFileChange) => f.staged_removed + f.unstaged_removed;

/** Changes tab: the session's working tree (`git.status`), diffs, commit selected files, discard with confirmation. */
export function ChangesTab({ sessionKey }: { sessionKey: string }) {
  const { host, id } = splitKey(sessionKey);
  const api = useApi();
  const actions = useActions();
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [error, setError] = useState("");
  const [sel, setSel] = useState<string>("");
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [diff, setDiff] = useState<GitDiff | null>(null);
  const [diffErr, setDiffErr] = useState("");
  const [split, setSplit] = useState(false);
  const [msg, setMsg] = useState("");
  const [confirm, setConfirm] = useState(false);
  const pending = useAppState((s) => s.pending);

  const load = useCallback(() => {
    setError("");
    api.git.status(host, { session: id }).then(
      (g) => {
        setStatus(g);
        setPicked(new Set(g.files.map((f) => f.path)));
        setSel((cur) => (g.files.some((f) => f.path === cur) ? cur : g.files[0]?.path ?? ""));
        actions.refreshGit(sessionKey);
      },
      (e) => setError(errorMessage(e)),
    );
  }, [api, host, id, actions, sessionKey]);
  useEffect(load, [load]);

  const file = status?.files.find((f) => f.path === sel);
  useEffect(() => {
    if (!file) return setDiff(null);
    let off = false;
    setDiffErr("");
    setDiff(null);
    api.git.diff(host, { target: { session: id }, path: file.path, side: file.unstaged ? "unstaged" : "staged", max_bytes: MAX_DIFF }).then(
      (d) => !off && setDiff(d),
      (e) => !off && setDiffErr(errorMessage(e)),
    );
    return () => {
      off = true;
    };
  }, [api, host, id, file]);

  const files = useMemo(() => status?.files ?? [], [status]);
  const chosen = files.filter((f) => picked.has(f.path)).map((f) => f.path);
  const toggle = (p: string) => setPicked((s) => {
    const n = new Set(s);
    if (n.has(p)) n.delete(p);
    else n.add(p);
    return n;
  });

  const commit = async () => {
    const r = await actions.run(`commit:${sessionKey}`, "Could not commit", () => api.git.commit(host, { target: { session: id }, message: msg.trim(), files: chosen }));
    if (!r) return;
    toast({ tone: "ok", title: `Committed ${chosen.length} file${chosen.length === 1 ? "" : "s"}`, detail: `${r.commit} on ${status?.branch ?? "detached HEAD"}` });
    setMsg("");
    load();
  };
  const discard = async () => {
    const r = await actions.run(`discard:${sessionKey}`, "Could not discard the changes", () => api.git.discard(host, { target: { session: id }, files: chosen }));
    setConfirm(false);
    if (!r) return;
    toast({ tone: "ok", title: `Discarded changes to ${chosen.length} file${chosen.length === 1 ? "" : "s"}` });
    load();
  };

  if (error) return <div className="placeholder"><EmptyState icon="branch" title="No changes to show" action={<Button size="sm" icon="refresh" onClick={load}>Try again</Button>}>{error}</EmptyState></div>;
  if (!status) return <div className="chg"><div className="flist"><div className="fl-pad"><Skeleton /><Skeleton width="70%" /><Skeleton width="80%" /></div></div><div /></div>;
  if (!files.length) return <div className="placeholder"><EmptyState icon="done" title="Working tree is clean" action={<Button size="sm" icon="refresh" onClick={load}>Refresh</Button>}>Nothing uncommitted in {status.repo_root} on {status.branch ?? "detached HEAD"}.</EmptyState></div>;
  const tot = files.reduce((a, f) => [a[0] + added(f), a[1] + removed(f)], [0, 0]);

  return (
    <div className="chg">
      <div className="flist">
        <div className="fl-h">
          <span className="h3">{files.length} files changed</span>
          <span className="mono"><span className="add">+{tot[0]}</span> <span className="del">−{tot[1]}</span></span>
          <span className="sp" />
          <Button size="sm" variant="ghost" disabled={!chosen.length} onClick={() => setConfirm(true)}>Discard…</Button>
        </div>
        <div className="scroll" role="listbox" aria-label="Changed files" aria-multiselectable="false">
          {files.map((f) => (
            <div key={f.path} role="option" aria-selected={f.path === sel} tabIndex={f.path === sel ? 0 : -1} className={`frow${f.path === sel ? " on" : ""}`} onClick={() => setSel(f.path)}
              onKeyDown={(e) => {
                const i = files.indexOf(f);
                if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                  e.preventDefault();
                  const n = files[Math.max(0, Math.min(files.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))];
                  setSel(n.path);
                  (e.currentTarget.parentElement?.children[files.indexOf(n)] as HTMLElement | undefined)?.focus();
                } else if (e.key === " ") {
                  e.preventDefault();
                  toggle(f.path);
                }
              }}>
              <input type="checkbox" className="fcb" checked={picked.has(f.path)} onChange={() => toggle(f.path)} onClick={(e) => e.stopPropagation()} aria-label={`Include ${f.path}`} />
              <span className={`ms ${letter(f) === "A" ? "add" : letter(f) === "D" ? "del" : "c-working"}`} title={f.unstaged ?? f.staged ?? ""}>{letter(f)}</span>
              <span className="fn" title={f.orig_path ? `${f.orig_path} → ${f.path}` : f.path}>{f.path}</span>
              {f.binary ? <span className="faint mono fs">bin</span> : <><span className="mono add fs">{added(f) ? `+${added(f)}` : ""}</span><span className="mono del fs">{removed(f) ? `−${removed(f)}` : ""}</span></>}
            </div>
          ))}
        </div>
        <div className="commit">
          <label className="eyebrow" htmlFor="commit-msg">Commit to {status.branch ?? "detached HEAD"}</label>
          <textarea id="commit-msg" className="ta" value={msg} onChange={(e) => setMsg(e.target.value)} placeholder="Describe the change" rows={3} />
          <div className="row-gap">
            <Button variant="primary" size="sm" icon="commit" disabled={!msg.trim() || !chosen.length} pending={!!pending[`commit:${sessionKey}`]} onClick={() => void commit()}>
              Commit {chosen.length} file{chosen.length === 1 ? "" : "s"}
            </Button>
            <Button size="sm" variant="ghost" icon="refresh" onClick={load}>Refresh</Button>
          </div>
        </div>
      </div>
      <div className="dpane">
        <div className="dp-h">
          <span className="mono dp-path">{file?.path}</span>
          {file && !file.binary ? <span className="mono"><span className="add">+{added(file)}</span> <span className="del">−{removed(file)}</span></span> : null}
          <span className="sp" />
          <Segmented label="Diff layout" value={split ? "split" : "unified"} onChange={(v) => setSplit(v === "split")} options={[{ value: "unified", label: "Unified" }, { value: "split", label: "Split" }]} />
        </div>
        <div className="scroll dp-b">
          {diffErr ? <div className="diff-note err" role="alert"><Icon name="failed" size="sm" />{diffErr}</div> : diff ? <DiffView diff={diff} split={split} /> : <div className="fl-pad"><Skeleton /><Skeleton width="60%" /></div>}
        </div>
      </div>
      <ConfirmDialog open={confirm} title={`Discard changes to ${chosen.length} file${chosen.length === 1 ? "" : "s"}?`} confirmLabel="Discard" danger pending={!!pending[`discard:${sessionKey}`]} onConfirm={() => void discard()} onClose={() => setConfirm(false)}>
        <p>Staged and unstaged changes are dropped and untracked files are deleted. This cannot be undone.</p>
        <ul className="mono confirm-list">{chosen.slice(0, 8).map((p) => <li key={p}>{p}</li>)}{chosen.length > 8 ? <li>… and {chosen.length - 8} more</li> : null}</ul>
      </ConfirmDialog>
    </div>
  );
}
