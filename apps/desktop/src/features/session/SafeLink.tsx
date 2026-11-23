import { createContext, type ReactNode } from "react";
import { errorMessage, type Api } from "../../bridge/api";
import { inShell } from "../../lib/shell";
import { useOptionalApi } from "../../state";
import { toast } from "../../ui/toast";

const ACTIVE = new Set(["http:", "https:", "mailto:"]);

/**
 * The link a piece of untrusted text may become: only http, https and mailto are active. Anything
 * else (javascript:, file:, data:, custom schemes, relative paths) is not a link. `open` is the
 * parsed form, so a shouted `HTTPS://` still reaches the shell as a URL and not as a file path.
 */
export function activeLink(href: string | undefined): { href: string; open: string } | null {
  const h = href?.trim();
  if (!h) return null;
  try {
    const u = new URL(h);
    return ACTIVE.has(u.protocol) ? { href: h, open: u.href } : null;
  } catch {
    return null;
  }
}

/** Hand a link to the system: the shell's native opener, `window.open` in a browser tab. */
async function openExternal(api: Api | null, url: string) {
  try {
    if (inShell() && api) await api.desktop.open(url);
    else window.open(url, "_blank", "noopener,noreferrer");
  } catch (e) {
    toast({ tone: "err", title: "Could not open the link", detail: errorMessage(e) });
  }
}

/** True under a link: nothing inside it may be interactive on its own. */
export const InLink = createContext(false);

/** A link in chat or file text: active only for http, https and mailto, otherwise plain text. */
export function SafeLink({ href, children }: { href?: string; children?: ReactNode }) {
  const api = useOptionalApi();
  const link = activeLink(href);
  if (!link) return <>{children}</>;
  return (
    <a
      href={link.href}
      target="_blank"
      rel="noreferrer noopener"
      onClick={(e) => {
        e.preventDefault();
        void openExternal(api, link.open);
      }}
    >
      <InLink.Provider value>{children}</InLink.Provider>
    </a>
  );
}
