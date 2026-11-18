import type { FsEntry, FsListRequest, FsListing } from "../wire";
import { err } from "./runtime";

/** Fixture-only: a small directory tree per host for the project picker. */
const TREE: Record<string, string[]> = {
  "/home/dev": ["src/", "Documents/", "Downloads/", ".config/", ".ssh/", "notes.md"],
  "/home/dev/src": ["ledgerline/*", "marsad/*", "docs-site/*", "tilemap-renderer/*", "scratch/"],
  "/home/dev/src/ledgerline": ["apps/", "packages/", "package.json", "pnpm-lock.yaml"],
  "/home/dev/src/marsad": ["apps/", "crates/", "Cargo.toml"],
  "/home/dev/src/docs-site": ["content/", "package.json"],
  "/home/dev/src/tilemap-renderer": ["src/", "Cargo.toml"],
  "/home/dev/src/scratch": [],
  "/home/dev/Documents": [],
  "/home/dev/Downloads": [],
};
const HOME = "/home/dev";

export const expand = (p: string) => (p === "~" ? HOME : p.startsWith("~/") ? `${HOME}/${p.slice(2)}` : p.replace(/\/+$/, "") || "/");
export const tilde = (p: string) => (p === HOME ? "~" : p.startsWith(HOME + "/") ? `~/${p.slice(HOME.length + 1)}` : p);

export function fsList(r: FsListRequest): FsListing {
  const path = expand(r.path);
  const kids = TREE[path];
  if (!kids) throw err("not_found", `No such directory: ${r.path}`, { marsad_error: "not_found", what: r.path });
  const entries: FsEntry[] = kids
    .map((k) => {
      const dir = k.endsWith("/") || k.endsWith("/*");
      const name = k.replace(/\/\*?$/, "");
      return { name, path: `${path}/${name}`, is_dir: dir, is_symlink: false, is_git_repo: k.endsWith("/*") };
    })
    .filter((e) => (r.show_hidden || !e.name.startsWith(".")) && (!r.dirs_only || e.is_dir))
    .sort((a, b) => Number(b.is_dir) - Number(a.is_dir) || a.name.localeCompare(b.name));
  const parent = path === "/" ? null : path.slice(0, path.lastIndexOf("/")) || "/";
  return { path, parent, entries, truncated: false };
}

export function fsExists(p: string) {
  const path = expand(p);
  const dir = path in TREE;
  const file = !dir && Object.entries(TREE).some(([d, ks]) => ks.some((k) => `${d}/${k}` === path));
  return { path, exists: dir || file, is_dir: dir };
}
export const fsHome = () => ({ path: HOME });
