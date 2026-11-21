import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { Code } from "../highlight/Code";
import { Markdown } from "../session/Markdown";
import { useApi } from "../../state";
import { Button, Dialog, Icon, Skeleton } from "../../ui";
import { toast } from "../../ui/toast";
import { b64ToBytes, bytesToB64, formatBytes, kindOf, readTarget, type Kind, type Loaded } from "./fetch";
import { closePreview, usePendingSecret, usePreview, type PreviewRequest } from "./preview";
import { dirnameOf, resolveHtmlResources, type Resource } from "./htmlResources";
import { htmlPreviewDoc, previewCsp } from "./sanitize";
import { DirListing } from "./DirListing";
import "./media.css";

const PdfView = lazy(() => import("./PdfView"));

/** What each kind loads at most (the host serves ranges; larger files show a notice). */
const LIMIT: Record<Kind, number> = {
  image: 32 << 20, svg: 4 << 20, pdf: 64 << 20, html: 2 << 20, markdown: 1 << 20, json: 2 << 20, csv: 2 << 20, text: 1 << 20, audio: 48 << 20, video: 96 << 20, other: 0,
};

interface HtmlResolver {
  baseDir: string;
  read: (abs: string, max: number) => Promise<Resource | null>;
}

/** Confirms, once per path, that a well-known secret location may be opened. */
export function SecretConfirm() {
  const p = usePendingSecret();
  if (!p) return null;
  return (
    <Dialog
      open
      onClose={() => p.done(false)}
      title="Open a file that may hold secrets?"
      width={520}
      footer={<><span className="sp" /><Button size="sm" onClick={() => p.done(false)}>Cancel</Button><Button size="sm" variant="primary" onClick={() => p.done(true)}>Open it</Button></>}
    >
      <p className="fs13">This looks like a place for keys or credentials:</p>
      <p className="mono fs12" data-testid="secret-path">{p.path}</p>
      <p className="faint fs12">It is shown only here, on your screen. Nothing is sent anywhere.</p>
    </Dialog>
  );
}

const decode = (b: Uint8Array) => new TextDecoder("utf-8", { fatal: false }).decode(b);

function useBlobUrl(l: Loaded | null, mime: string) {
  const url = useMemo(() => (l ? URL.createObjectURL(new Blob([l.bytes as BlobPart], { type: mime })) : null), [l, mime]);
  useEffect(() => () => void (url && URL.revokeObjectURL(url)), [url]);
  return url;
}

/** Fit to the panel (small images scale up) or actual size; click toggles. */
function ZoomImage({ url, alt }: { url: string; alt: string }) {
  const [actual, setActual] = useState(false);
  return (
    <div className={`pv-img${actual ? " actual" : ""}`}>
      <img src={url} alt={alt} data-testid="preview-image" title={actual ? "Click to fit" : "Click for actual size"} onClick={() => setActual((a) => !a)} />
    </div>
  );
}

/** A text preview: syntax colouring by name / type, line numbers, soft-wrap toggle. */
function CodeView({ text, name, mime, lang, line, note }: { text: string; name: string; mime?: string; lang?: string; line?: number | null; note?: string }) {
  const [wrap, setWrap] = useState(false);
  return (
    <div className="pv-text">
      <div className="pv-bar">
        <label className="pv-toggle"><input type="checkbox" checked={wrap} onChange={(e) => setWrap(e.target.checked)} /> Wrap lines</label>
        {note ? <span className="faint fs12">{note}</span> : null}
      </div>
      <div className="pv-code">
        <Code text={text} lang={lang} name={name} mime={mime} lineNumbers wrap={wrap} focusLine={line} testId="preview-text" />
      </div>
    </div>
  );
}

function JsonView({ text, name }: { text: string; name: string }) {
  const pretty = useMemo(() => {
    try {
      return JSON.stringify(JSON.parse(text), null, 2);
    } catch {
      return null;
    }
  }, [text]);
  return <CodeView text={pretty ?? text} name={name} lang="json" note={pretty == null ? "Not valid JSON; shown as text." : undefined} />;
}

/** Markdown files: formatted by default, with the source one click away. */
function MarkdownView({ text, name, line }: { text: string; name: string; line?: number | null }) {
  const [mode, setMode] = useState<"rendered" | "source">(line ? "source" : "rendered");
  return (
    <div className="pv-text">
      <div className="pv-bar" role="group" aria-label="Markdown view">
        <button type="button" className={`pv-seg${mode === "rendered" ? " on" : ""}`} aria-pressed={mode === "rendered"} onClick={() => setMode("rendered")}>Rendered</button>
        <button type="button" className={`pv-seg${mode === "source" ? " on" : ""}`} aria-pressed={mode === "source"} onClick={() => setMode("source")}>Source</button>
      </div>
      {mode === "rendered" ? <Markdown text={text} variant="file" /> : <div className="pv-code"><Code text={text} lang="markdown" name={name} lineNumbers focusLine={line} testId="preview-text" /></div>}
    </div>
  );
}

/** RFC 4180-ish: quoted fields, doubled quotes, `,` or tab separators. */
export function parseCsv(text: string, sep: string, maxRows = 500): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let f = "";
  let q = false;
  for (let i = 0; i < text.length && rows.length < maxRows; i++) {
    const c = text[i];
    if (q) {
      if (c === '"' && text[i + 1] === '"') { f += '"'; i++; } else if (c === '"') q = false; else f += c;
    } else if (c === '"' && f === "") q = true;
    else if (c === sep) { row.push(f); f = ""; }
    else if (c === "\n" || c === "\r") {
      if (c === "\r" && text[i + 1] === "\n") i++;
      row.push(f); rows.push(row); row = []; f = "";
    } else f += c;
  }
  if ((f || row.length) && rows.length < maxRows) { row.push(f); rows.push(row); }
  return rows;
}

function CsvView({ text, tsv }: { text: string; tsv: boolean }) {
  const rows = useMemo(() => parseCsv(text, tsv ? "\t" : ","), [text, tsv]);
  const [head, ...body] = rows;
  return (
    <div className="pv-table-wrap">
      <table className="pv-table" data-testid="preview-table">
        {head ? <thead><tr>{head.map((h, i) => <th key={i}>{h}</th>)}</tr></thead> : null}
        <tbody>{body.map((r, i) => <tr key={i}>{r.map((c, j) => <td key={j}>{c}</td>)}</tr>)}</tbody>
      </table>
      {rows.length >= 500 ? <div className="faint fs12">First 500 rows.</div> : null}
    </div>
  );
}

function HtmlView({ text, resolver }: { text: string; resolver: HtmlResolver | null }) {
  const [scripts, setScripts] = useState(false);
  const [resolved, setResolved] = useState<{ html: string; blocked: number; missing: number; text: string; scripts: boolean } | null>(null);
  const memo = useRef(new Map<string, Promise<Resource | null>>());
  const fallback = useMemo(() => htmlPreviewDoc(text, scripts), [text, scripts]);
  useEffect(() => {
    if (!resolver) return;
    let live = true;
    resolveHtmlResources(text, { baseDir: resolver.baseDir, scripts, read: resolver.read, csp: previewCsp(scripts), memo: memo.current }).then(
      (r) => live && setResolved({ ...r, text, scripts }),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [text, scripts, resolver]);
  const ready = resolved && resolved.text === text && resolved.scripts === scripts ? resolved : null;
  return (
    <div className="pv-html">
      <div className="pv-bar">
        <label className="pv-toggle" title="Scripts run in an isolated frame with no network and no access to Marsad.">
          <input type="checkbox" checked={scripts} onChange={(e) => setScripts(e.target.checked)} /> Run scripts
        </label>
        <span className="faint fs12">{scripts ? "Scripts on: isolated frame, no network, no access to Marsad." : "Scripts are off. Network access is always blocked."}</span>
        {ready && ready.blocked ? <span className="pv-note" data-testid="html-blocked">Remote content blocked: {ready.blocked}</span> : null}
        {ready && ready.missing ? <span className="faint fs12" data-testid="html-missing">{ready.missing} local resource{ready.missing > 1 ? "s" : ""} could not be loaded</span> : null}
      </div>
      {/* No allow-same-origin: the frame's origin is opaque, so it can reach neither the app nor the bridge. */}
      <iframe key={scripts ? "s" : "n"} title="HTML preview" data-testid="preview-html" sandbox={scripts ? "allow-scripts" : ""} srcDoc={ready?.html ?? fallback} referrerPolicy="no-referrer" />
    </div>
  );
}

function Body({ req, kind, data, resolver }: { req: PreviewRequest; kind: Kind; data: Loaded; resolver: HtmlResolver | null }) {
  const mime = kind === "svg" ? "image/svg+xml" : data.mime;
  const url = useBlobUrl(kind === "image" || kind === "svg" || kind === "audio" || kind === "video" ? data : null, mime);
  const text = useMemo(() => (["html", "markdown", "json", "csv", "text"].includes(kind) ? decode(data.bytes) : ""), [kind, data]);
  switch (kind) {
    case "image":
    case "svg":
      // An <img> never runs an SVG's scripts or loads its external references.
      return url ? <ZoomImage url={url} alt={req.name} /> : null;
    case "pdf":
      return <Suspense fallback={<Skeleton />}><PdfView bytes={data.bytes} name={req.name} /></Suspense>;
    case "html":
      return <HtmlView text={text} resolver={resolver} />;
    case "markdown":
      return <MarkdownView text={text} name={req.name} line={req.line} />;
    case "json":
      return <JsonView text={text} name={req.name} />;
    case "csv":
      return <CsvView text={text} tsv={data.mime === "text/tab-separated-values" || req.name.toLowerCase().endsWith(".tsv")} />;
    case "text":
      return <CodeView text={text} name={req.name} mime={data.mime} line={req.line} />;
    case "audio":
      return url ? <audio controls src={url} data-testid="preview-audio" /> : null;
    case "video":
      return url ? <video controls src={url} className="pv-video" data-testid="preview-video" /> : null;
    default:
      return null;
  }
}

/** The preview panel: one file at a time, every kind rendered as untrusted content. */
export function PreviewHost() {
  const req = usePreview();
  const api = useApi();
  const [state, setState] = useState<{ data?: Loaded; error?: string; meta?: { size: number; mime: string; path: string } }>({});
  const [busy, setBusy] = useState(false);
  const name = req?.name ?? "";
  const kind = req ? kindOf(state.meta?.mime ?? req.mime, name) : "other";
  useEffect(() => {
    setState({});
    if (!req || req.dir) return;
    let live = true;
    // The first range tells the host-sniffed type and size; then load what the kind needs.
    (async () => {
      const head = await readTarget(api, req.host, req.session, req.target, 64 * 1024);
      const k = kindOf(head.mime, req.name);
      const meta = { size: head.size, mime: head.mime, path: head.path };
      if (!live) return;
      if (k === "other") return setState({ meta });
      const data = head.truncated && LIMIT[k] > head.bytes.length ? await readTarget(api, req.host, req.session, req.target, LIMIT[k]) : head;
      if (live) setState({ data, meta });
    })().catch((e: unknown) => live && setState({ error: (e as { message?: string })?.message ?? String(e) }));
    return () => {
      live = false;
    };
  }, [req, api]);

  const resolver = useMemo<HtmlResolver | null>(() => {
    const path = state.meta?.path;
    if (!req || !path) return null;
    const t = req.target;
    return {
      baseDir: dirnameOf(path),
      // Each resource is read through the same restricted method; a file the user opened from
      // outside the session's folder may load only what lies in its own directory (the host
      // checks the grant against the canonical path).
      read: async (abs, max) => {
        try {
          const target = "path" in t && t.outside ? { path: abs, outside: true, grant: (await api.media.grant(req.host, { id: req.session, path: abs, parent_grant: t.grant ?? null })).token } : { path: abs };
          const r = await readTarget(api, req.host, req.session, target, max);
          return r.truncated ? null : { bytes: r.bytes, mime: r.mime };
        } catch {
          return null;
        }
      },
    };
  }, [req, api, state.meta?.path]);

  if (!req) return <SecretConfirm />;
  if (req.dir) return <><Dialog open onClose={closePreview} title={name} subtitle="Folder" className="preview-dlg" width={720}><DirListing req={req} /></Dialog><SecretConfirm /></>;
  const meta = state.meta;
  const download = async (open: boolean) => {
    setBusy(true);
    try {
      const { path } = await api.desktop.downloadBegin(name);
      let offset = 0;
      for (;;) {
        const c = await api.media.read(req.host, { id: req.session, blob: "blob" in req.target ? req.target.blob : null, path: "path" in req.target ? req.target.path : null, offset, length: 2 << 20, outside: "path" in req.target ? !!req.target.outside : false, grant: "path" in req.target ? req.target.grant ?? null : null });
        await api.desktop.downloadChunk(path, c.data);
        offset = c.offset + b64ToBytes(c.data).length;
        if (c.eof) break;
      }
      toast({ tone: "ok", title: "Downloaded", detail: path });
      if (open) await api.desktop.open(path);
    } catch (e) {
      toast({ tone: "err", title: "Could not download the file", detail: (e as { message?: string })?.message ?? String(e) });
    } finally {
      setBusy(false);
    }
  };
  const nativeAction = (f: (p: string, distro?: string | null) => Promise<unknown>, what: string) => () =>
    meta && f(meta.path, req.distro).catch((e: unknown) => toast({ tone: "err", title: `Could not ${what}`, detail: (e as { message?: string })?.message ?? String(e) }));
  const footer = (
    <>
      {meta ? <span className="faint fs12 pv-meta" title={meta.path}>{meta.mime} · {formatBytes(meta.size)}{state.data?.truncated ? ` · showing the first ${formatBytes(state.data.bytes.length)}` : ""}</span> : null}
      <span className="sp" />
      {req.localFiles ? (
        <>
          <Button size="sm" icon="eye" disabled={!meta} onClick={nativeAction(api.desktop.open, "open the file")}>Open with system app</Button>
          <Button size="sm" icon="folder" disabled={!meta} onClick={nativeAction(api.desktop.reveal, "reveal the file")}>Reveal in folder</Button>
        </>
      ) : (
        <>
          <Button size="sm" icon="download" pending={busy} disabled={!meta} onClick={() => void download(false)}>Download</Button>
          <Button size="sm" icon="eye" disabled={!meta || busy} onClick={() => void download(true)}>Download and open</Button>
        </>
      )}
    </>
  );
  return (
    <>
    <Dialog open onClose={closePreview} title={name} subtitle={meta ? undefined : "Loading…"} className="preview-dlg" width={1100} footer={footer}>
      <div className="pv-body" data-testid="preview" data-kind={kind}>
        {state.error ? <div className="c-failed" role="alert"><Icon name="warn" size="sm" /> {state.error}</div> : null}
        {!state.error && !meta ? <><Skeleton width="60%" /><Skeleton /><Skeleton width="80%" /></> : null}
        {meta && kind === "other" ? (
          <div className="pv-other" data-testid="preview-other">
            <Icon name="file" />
            <div><b>{name}</b></div>
            <div className="faint">{meta.mime} · {formatBytes(meta.size)}</div>
            <div className="faint fs12">No preview for this type.{req.localFiles ? "" : " Download it to open it with an app on this computer."}</div>
          </div>
        ) : null}
        {state.data ? <Body req={req} kind={kind} data={state.data} resolver={resolver} /> : null}
      </div>
    </Dialog>
    <SecretConfirm />
    </>
  );
}

/** Exposed for tests. */
export const __test = { bytesToB64 };
