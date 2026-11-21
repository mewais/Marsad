/**
 * HTML previews: the file is untrusted. It is shown in a sandboxed iframe (opaque origin: no
 * access to the app, its storage or the desktop bridge) under a Content-Security-Policy that
 * forbids every network load; scripts run only when the user turns them on.
 */
export function previewCsp(scripts: boolean): string {
  return [
    "default-src 'none'",
    "img-src data: blob:",
    "media-src data: blob:",
    "font-src data:",
    "style-src 'unsafe-inline'",
    scripts ? "script-src 'unsafe-inline'" : "script-src 'none'",
    "form-action 'none'",
    "base-uri 'none'",
    "frame-src 'none'",
    "connect-src 'none'",
  ].join("; ");
}

/** The `srcdoc` of an HTML preview: our CSP first (a later policy can only tighten it). */
export function htmlPreviewDoc(html: string, scripts: boolean): string {
  const meta = `<meta http-equiv="Content-Security-Policy" content="${previewCsp(scripts)}"><meta name="referrer" content="no-referrer">`;
  return `<!doctype html><html><head>${meta}</head><body>${html}</body></html>`;
}
