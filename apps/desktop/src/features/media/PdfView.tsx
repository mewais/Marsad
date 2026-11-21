import { useEffect, useRef, useState } from "react";
import { Button } from "../../ui";

/** Pages rendered at most (the rest on demand). */
const PAGE_STEP = 10;

/**
 * A PDF rendered with pdf.js (this module and pdf.js are a separate chunk, loaded on first
 * use). Scripting, forms and external links stay off: the document is untrusted.
 */
export default function PdfView({ bytes, name }: { bytes: Uint8Array; name: string }) {
  const box = useRef<HTMLDivElement>(null);
  const [pages, setPages] = useState(0);
  const [shown, setShown] = useState(PAGE_STEP);
  const [error, setError] = useState<string | null>(null);
  const doc = useRef<import("pdfjs-dist").PDFDocumentProxy | null>(null);

  useEffect(() => {
    let live = true;
    let task: import("pdfjs-dist").PDFDocumentLoadingTask | null = null;
    (async () => {
      const pdfjs = await import("pdfjs-dist/legacy/build/pdf.mjs");
      const worker = (await import("pdfjs-dist/legacy/build/pdf.worker.min.mjs?url")).default;
      pdfjs.GlobalWorkerOptions.workerSrc = worker;
      task = pdfjs.getDocument({ data: bytes.slice(), isEvalSupported: false, enableXfa: false, disableFontFace: false, stopAtErrors: false });
      const d = await task.promise;
      if (!live) return void d.destroy();
      doc.current = d;
      setPages(d.numPages);
    })().catch((e: unknown) => live && setError((e as { message?: string })?.message ?? String(e)));
    return () => {
      live = false;
      void task?.destroy();
      doc.current = null;
    };
  }, [bytes]);

  useEffect(() => {
    const d = doc.current;
    const el = box.current;
    if (!d || !el || !pages) return;
    let live = true;
    (async () => {
      const width = Math.max(320, el.clientWidth - 24);
      for (let n = el.childElementCount + 1; n <= Math.min(pages, shown) && live; n++) {
        const page = await d.getPage(n);
        const base = page.getViewport({ scale: 1 });
        const scale = Math.min(2, width / base.width);
        const vp = page.getViewport({ scale: scale * (window.devicePixelRatio || 1) });
        const canvas = document.createElement("canvas");
        canvas.width = vp.width;
        canvas.height = vp.height;
        canvas.style.width = `${vp.width / (window.devicePixelRatio || 1)}px`;
        canvas.setAttribute("aria-label", `${name}, page ${n}`);
        canvas.dataset.page = String(n);
        const ctx = canvas.getContext("2d");
        if (!ctx || !live) return;
        el.appendChild(canvas);
        await page.render({ canvasContext: ctx, viewport: vp }).promise;
      }
    })().catch((e: unknown) => live && setError((e as { message?: string })?.message ?? String(e)));
    return () => {
      live = false;
    };
  }, [pages, shown, name]);

  return (
    <div className="pv-pdf" data-testid="preview-pdf" data-pages={pages || undefined}>
      {error ? <div className="c-failed" role="alert">Could not render the PDF: {error}</div> : null}
      <div ref={box} className="pv-pages" />
      {pages > shown ? <Button size="sm" onClick={() => setShown((s) => s + PAGE_STEP)}>Show more pages ({pages - shown} left)</Button> : null}
    </div>
  );
}
