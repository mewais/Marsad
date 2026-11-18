import { createContext, useContext, useEffect, useMemo } from "react";
import { createStore, useStore, type Store } from "../lib/store";

/** Tiny router: a path in a store, bound to the URL hash (app) or kept in memory (tests). */
export interface History {
  store: Store<string>;
  push(path: string): void;
  dispose(): void;
}

export function hashHistory(): History {
  const read = () => (location.hash.replace(/^#/, "") || "/inbox");
  const store = createStore(read());
  const on = () => store.set(read());
  window.addEventListener("hashchange", on);
  return {
    store,
    push: (p) => {
      if (location.hash.replace(/^#/, "") !== p) location.hash = p;
      else store.set(p);
    },
    dispose: () => window.removeEventListener("hashchange", on),
  };
}

export function memoryHistory(initial = "/inbox"): History {
  const store = createStore(initial);
  return { store, push: (p) => store.set(p), dispose: () => undefined };
}

export type Route =
  | { name: "inbox" }
  | { name: "session"; host: string; id: string; tab: string }
  | { name: "workstreams" }
  | { name: "hosts" }
  | { name: "config" }
  | { name: "settings" }
  | { name: "notfound"; path: string };

export function parseRoute(path: string): Route {
  const [p, qs = ""] = path.split("?");
  const seg = p.split("/").filter(Boolean);
  switch (seg[0]) {
    case undefined:
    case "inbox":
      return { name: "inbox" };
    case "session":
      if (seg[1] && seg[2]) return { name: "session", host: decodeURIComponent(seg[1]), id: decodeURIComponent(seg[2]), tab: new URLSearchParams(qs).get("tab") ?? "activity" };
      return { name: "notfound", path };
    case "workstreams":
    case "hosts":
    case "config":
    case "settings":
      return { name: seg[0] };
    default:
      return { name: "notfound", path };
  }
}

export const sessionPath = (host: string, id: string, tab?: string) =>
  `/session/${encodeURIComponent(host)}/${encodeURIComponent(id)}${tab && tab !== "activity" ? `?tab=${tab}` : ""}`;

const Ctx = createContext<History | null>(null);

export function RouterProvider({ history, children }: { history: History; children: React.ReactNode }) {
  useEffect(() => () => history.dispose(), [history]);
  return <Ctx.Provider value={history}>{children}</Ctx.Provider>;
}

function useHistory(): History {
  const h = useContext(Ctx);
  if (!h) throw new Error("RouterProvider missing");
  return h;
}

export function useRoute(): Route {
  const h = useHistory();
  const path = useStore(h.store, (s) => s);
  return useMemo(() => parseRoute(path), [path]);
}

export const useNavigate = () => useHistory().push;

export function Link({ to, children, className, ...rest }: { to: string } & Omit<React.AnchorHTMLAttributes<HTMLAnchorElement>, "href">) {
  const nav = useNavigate();
  return (
    <a
      href={`#${to}`}
      className={className}
      onClick={(e) => {
        if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
        e.preventDefault();
        nav(to);
      }}
      {...rest}
    >
      {children}
    </a>
  );
}
