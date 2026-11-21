import { lazy, Suspense, useEffect, useMemo, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { useApi } from "../../state";
import { Button, Dialog, Icon, Skeleton } from "../../ui";
import { toast } from "../../ui/toast";
import { b64ToBytes, bytesToB64, formatBytes, kindOf, readTarget, type Kind, type Loaded } from "./fetch";
import { closePreview, usePreview, type PreviewRequest } from "./preview";
import { htmlPreviewDoc } from "./sanitize";
import "./media.css";

const PdfView = lazy(() => import("./PdfView"));

/** What each kind loads at most (the host serves ranges; larger files show a notice). */
const LIMIT: Record<Kind, number> = {
  image: 32 << 20, svg: 4 << 20, pdf: 64 << 20, html: 2 << 20, markdown: 1 << 20, json: 2 << 20, csv: 2 << 20, text: 1 << 20, audio: 48 << 20, video: 96 << 20, other: 0,
};

const decode = (b: Uint8Array) => new TextDecoder("utf-8", { fatal: false }).decode(b);

function useBlobUrl(l: Loaded | null, mime: string) {
  const url = useMemo(() => (l ? URL.createObjectURL(new Blob([l.bytes as BlobPart], { type: mime })) : null), [l, mime]);
  useEffect(() => () => void (url && URL.revokeObjectURL(url)), [url]);
  return url;
}

function TextView({ text }: { text: string }) {
  const lines = text.split("\n");
  return (
    <pre className="pv-code" data-testid="preview-text">
      {lines.map((l, i) => (
        <div key={i} className="pv-ln"><span className="pv-no">{i + 1}</span><span>{l || " "}</span></div>
      ))}
    </pre>
  );
}

function JsonView({ text }: { text: string }) {
  const pretty = useMemo(() => {
    try {
      return JSON.stringify(JSON.parse(text), null, 2);
    } catch {
      return null;
    }
  }, [text]);
  return pretty == null ? (
    <><div className="faint fs12">Not valid JSON; shown as text.</div><TextView text={text} /></>
  ) : (
    <TextView text={pretty} />
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

/** Markdown from an untrusted file: no raw HTML, no remote images, links inert. */
function SafeMarkdown({ text }: { text: string }) {
  return (
    <div className="md pv-md" data-testid="preview-markdown">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        skipHtml
        components={{
          img: ({ alt }) => <span className="faint">[image{alt ? `: ${alt}` : ""}]</span>,
          a: ({ children, href }) => <span className="pv-link" title={href}>{children}</span>,
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}

function HtmlView({ text }: { text: string }) {
  const [scripts, setScripts] = useState(false);
  const doc = useMemo(() => htmlPreviewDoc(text, scripts), [text, scripts]);
  return (
    <div className="pv-html">
      <div className="pv-bar">
        <label className="pv-toggle" title="Scripts run in an isolated frame with no network and no access to Marsad.">
          <input type="checkbox" checked={scripts} onChange={(e) => setScripts(e.target.checked)} /> Run scripts
        </label>
        <span className="faint fs12">{scripts ? "Scripts on: isolated frame, no network, no access to Marsad." : "Scripts are off. Network access is always blocked."}</span>
      </div>
      {/* No allow-same-origin: the frame's origin is opaque, so it can reach neither the app nor the bridge. */}
      <iframe key={scripts ? "s" : "n"} title="HTML preview" data-testid="preview-html" sandbox={scripts ? "allow-scripts" : ""} srcDoc={doc} referrerPolicy="no-referrer" />
    </div>
  );
}

function Body({ req, kind, data }: { req: PreviewRequest; kind: Kind; data: Loaded }) {
  const mime = kind === "svg" ? "image/svg+xml" : data.mime;
  const url = useBlobUrl(kind === "image" || kind === "svg" || kind === "audio" || kind === "video" ? data : null, mime);
  const text = useMemo(() => (["html", "markdown", "json", "csv", "text"].includes(kind) ? decode(data.bytes) : ""), [kind, data]);
  switch (kind) {
    case "image":
    case "svg":
      // An <img> never runs an SVG's scripts or loads its external references.
      return url ? <div className="pv-img"><img src={url} alt={req.name} data-testid="preview-image" /></div> : null;
    case "pdf":
      return <Suspense fallback={<Skeleton />}><PdfView bytes={data.bytes} name={req.name} /></Suspense>;
    case "html":
      return <HtmlView text={text} />;
    case "markdown":
      return <SafeMarkdown text={text} />;
    case "json":
      return <JsonView text={text} />;
    case "csv":
      return <CsvView text={text} tsv={data.mime === "text/tab-separated-values" || req.name.toLowerCase().endsWith(".tsv")} />;
    case "text":
      return <TextView text={text} />;
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
    if (!req) return;
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

  if (!req) return null;
  const meta = state.meta;
  const download = async (open: boolean) => {
    setBusy(true);
    try {
      const { path } = await api.desktop.downloadBegin(name);
      let offset = 0;
      for (;;) {
        const c = await api.media.read(req.host, { id: req.session, blob: "blob" in req.target ? req.target.blob : null, path: "path" in req.target ? req.target.path : null, offset, length: 2 << 20, outside: false });
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
  const nativeAction = (f: (p: string) => Promise<unknown>, what: string) => () =>
    meta && f(meta.path).catch((e: unknown) => toast({ tone: "err", title: `Could not ${what}`, detail: (e as { message?: string })?.message ?? String(e) }));
  const footer = (
    <>
      {meta ? <span className="faint fs12 pv-meta" title={meta.path}>{meta.mime} · {formatBytes(meta.size)}{state.data?.truncated ? ` · showing the first ${formatBytes(state.data.bytes.length)}` : ""}</span> : null}
      <span className="sp" />
      {req.local ? (
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
    <Dialog open onClose={closePreview} title={name} subtitle={meta ? undefined : "Loading…"} className="preview-dlg" width={1100} footer={footer}>
      <div className="pv-body" data-testid="preview" data-kind={kind}>
        {state.error ? <div className="c-failed" role="alert"><Icon name="warn" size="sm" /> {state.error}</div> : null}
        {!state.error && !meta ? <><Skeleton width="60%" /><Skeleton /><Skeleton width="80%" /></> : null}
        {meta && kind === "other" ? (
          <div className="pv-other" data-testid="preview-other">
            <Icon name="file" />
            <div><b>{name}</b></div>
            <div className="faint">{meta.mime} · {formatBytes(meta.size)}</div>
            <div className="faint fs12">No preview for this type.{req.local ? "" : " Download it to open it with an app on this computer."}</div>
          </div>
        ) : null}
        {state.data ? <Body req={req} kind={kind} data={state.data} /> : null}
      </div>
    </Dialog>
  );
}

/** Exposed for tests. */
export const __test = { bytesToB64 };
