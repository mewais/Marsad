import { createContext, useContext, useEffect, useRef, useState } from "react";
import type { Attachment, ContentPart, HostId, SessionId } from "../../bridge/wire";
import { useApi } from "../../state";
import { Icon } from "../../ui";
import { baseName, formatBytes, loadCached, targetOf, type Target } from "./fetch";
import { openPreview } from "./preview";

/** Which session the media in a transcript belongs to (reads go to its host). */
export interface MediaScope {
  host: HostId;
  session: SessionId;
  local: boolean;
}

export const MediaCtx = createContext<MediaScope | null>(null);
export const useMediaScope = () => useContext(MediaCtx);

/** Thumbnails load at most this much (the lightbox loads the full image). */
const THUMB_MAX = 8 * 1024 * 1024;

/** An object URL for the bytes of `target`, loaded when `active`. */
export function useObjectUrl(target: Target | null, mime: string | undefined, active: boolean, max = THUMB_MAX) {
  const api = useApi();
  const scope = useMediaScope();
  const [state, setState] = useState<{ url?: string; error?: string }>({});
  const key = target ? JSON.stringify(target) : "";
  useEffect(() => {
    if (!active || !target || !scope) return;
    let url: string | undefined;
    let live = true;
    loadCached(api, scope.host, scope.session, target, max).then(
      (l) => {
        if (!live) return;
        // SVG is shown through <img>, where its scripts never run.
        url = URL.createObjectURL(new Blob([l.bytes as BlobPart], { type: mime || l.mime }));
        setState({ url });
      },
      (e: unknown) => live && setState({ error: (e as { message?: string })?.message ?? String(e) }),
    );
    return () => {
      live = false;
      if (url) URL.revokeObjectURL(url);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, active, scope?.host, scope?.session, mime, max]);
  return state;
}

/** `true` once the element scrolled near the viewport (immediately without IntersectionObserver). */
function useNearViewport<T extends Element>(): [React.RefObject<T>, boolean] {
  const ref = useRef<T>(null);
  const [near, setNear] = useState(typeof IntersectionObserver === "undefined");
  useEffect(() => {
    if (near || !ref.current) return;
    const io = new IntersectionObserver((es) => es.some((e) => e.isIntersecting) && setNear(true), { rootMargin: "400px" });
    io.observe(ref.current);
    return () => io.disconnect();
  }, [near]);
  return [ref, near];
}

/** An inline image: a lazily loaded, size-capped thumbnail that opens the lightbox. */
export function MediaImage({ target, mime, alt, width, height, size }: { target: Target; mime: string; alt: string; width?: number | null; height?: number | null; size?: number | null }) {
  const scope = useMediaScope();
  const [ref, near] = useNearViewport<HTMLButtonElement>();
  const { url, error } = useObjectUrl(target, mime, near);
  const ratio = width && height ? `${width} / ${height}` : undefined;
  return (
    <button
      ref={ref}
      type="button"
      className="media-thumb plainbtn"
      data-testid="media-image"
      style={ratio ? { aspectRatio: ratio } : undefined}
      title={`${alt}${width && height ? ` · ${width}×${height}` : ""}${size ? ` · ${formatBytes(size)}` : ""}`}
      onClick={() => scope && openPreview({ ...scope, target, name: alt, mime, size: size ?? null })}
    >
      {url ? <img src={url} alt={alt} loading="lazy" decoding="async" draggable={false} /> : error ? <span className="media-err fs12">Image unavailable: {error}</span> : <span className="media-ph" aria-label={`Loading ${alt}`} />}
    </button>
  );
}

const fileIcon = (mime?: string | null) => (mime?.startsWith("image/") ? "eye" : "file");

/** A file chip: name, type and size; click previews it. `note` explains how it was sent. */
export function FileChip({ target, name, mime, size, note, onRemove, testId = "file-chip" }: { target: Target | null; name: string; mime?: string | null; size?: number | null; note?: string; onRemove?: () => void; testId?: string }) {
  const scope = useMediaScope();
  const can = !!(target && scope);
  const label = `${name}${size != null ? ` · ${formatBytes(size)}` : ""}`;
  return (
    <span className="fchip" data-testid={testId} title={note ? `${label}\n${note}` : label}>
      <button type="button" className="plainbtn fchip-b" disabled={!can} onClick={() => can && openPreview({ ...scope!, target: target!, name, mime: mime ?? null, size: size ?? null })}>
        <Icon name={fileIcon(mime)} size="xs" />
        <span className="fchip-n">{name}</span>
        {size != null ? <span className="faint fchip-s">{formatBytes(size)}</span> : null}
      </button>
      {onRemove ? (
        <button type="button" className="plainbtn fchip-x" aria-label={`Remove ${name}`} onClick={onRemove}>
          <Icon name="x" size="xs" />
        </button>
      ) : null}
    </span>
  );
}

/** Images inline, every other part as a file chip; text parts as plain text. */
export function PartsView({ parts }: { parts: ContentPart[] }) {
  if (!parts.length) return null;
  return (
    <div className="parts" data-testid="parts">
      {parts.map((p, i) => {
        if (p.type === "text") return <div key={i} className="faint fs12">{p.text}</div>;
        const t = targetOf(p.source);
        const size = p.source.source === "blob" ? p.source.size : null;
        if (p.type === "image") {
          const alt = p.alt ?? (p.source.source === "path" ? baseName(p.source.path) : "image");
          if (t) return <MediaImage key={i} target={t} mime={p.mime} alt={alt} width={p.width} height={p.height} size={size} />;
          return <span key={i} className="fchip faint" title={p.source.source === "url" ? p.source.url : undefined}><Icon name="eye" size="xs" /> {alt} (remote image, not loaded)</span>;
        }
        const name = p.name ?? (p.source.source === "path" ? baseName(p.source.path) : p.uri ? baseName(p.uri) : "file");
        const note = p.source.source === "url" ? `Remote resource ${p.source.url} (not fetched)` : undefined;
        return <FileChip key={i} target={t} name={name} mime={p.mime} size={size} note={note} />;
      })}
    </div>
  );
}

/** A user's attachments as sent: images as thumbnails, other files as chips. */
export function AttachmentsView({ attachments }: { attachments: Attachment[] }) {
  if (!attachments.length) return null;
  return (
    <div className="parts" data-testid="sent-attachments">
      {attachments.map((a, i) =>
        a.mime.startsWith("image/") && a.mime !== "image/svg+xml" ? (
          <MediaImage key={i} target={{ path: a.path }} mime={a.mime} alt={a.name} size={a.size} />
        ) : (
          <FileChip key={i} target={{ path: a.path }} name={a.name} mime={a.mime} size={a.size} />
        ),
      )}
    </div>
  );
}
