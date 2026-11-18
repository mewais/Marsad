import type { GitDiff, GitFileChange, GitStatus } from "../wire";
import type { Spec } from "./sessions";

/** Fixture-only: working trees per session (git.status / git.diff / git.commit / git.discard). */

const SERVER_DIFF = `diff --git a/apps/api/src/server.ts b/apps/api/src/server.ts
index 3f1c2e0..9ab44d1 100644
--- a/apps/api/src/server.ts
+++ b/apps/api/src/server.ts
@@ -1,8 +1,8 @@
 import express from 'express'
 import helmet from 'helmet'
-import session from 'express-session'
-import RedisStore from 'connect-redis'
+import { authMiddleware } from '@ledgerline/auth'
+import { legacySessionCompat } from '@ledgerline/auth/legacy-compat'
 import { client } from './redis'
 import { env } from './env'
 import { routes } from './routes'
 import { errorHandler } from './errors'
@@ -18,10 +18,12 @@ const app = express()
 app.use(helmet())
-app.use(session({ store: new RedisStore({ client }), secret: env.SESSION_SECRET }))
-app.use(passport.session())
+app.use(legacySessionCompat({ client, until: env.LEGACY_COOKIE_UNTIL }))
+app.use(authMiddleware({ issuer: env.AUTH_ISSUER, cookie: 'll_session' }))
+
+// TODO(auth-mw): drop compat after LEGACY_COOKIE_UNTIL
 app.use('/v1', routes)
 app.use(errorHandler)
 
 export default app
`;

const f = (path: string, kind: GitFileChange["unstaged"], added: number, removed: number, binary = false): GitFileChange => ({
  path, orig_path: null, staged: null, unstaged: kind, staged_added: 0, staged_removed: 0, unstaged_added: added, unstaged_removed: removed, binary,
});

export interface Tree {
  status: GitStatus;
  diffs: Record<string, string>;
}

function genericDiff(path: string, added: number, removed: number): string {
  const lines = [`diff --git a/${path} b/${path}`, `--- a/${path}`, `+++ b/${path}`, `@@ -10,${removed + 3} +10,${added + 3} @@ export function main() {`, "   const cfg = load()"];
  for (let i = 0; i < removed; i++) lines.push(`-  legacy${i}(cfg)`);
  for (let i = 0; i < added; i++) lines.push(`+  step${i}(cfg, { bounded: true })`);
  lines.push("   return cfg", " }");
  return lines.join("\n") + "\n";
}

export function buildTree(spec: Spec): Tree {
  const repo = `/home/dev/src/${spec.proj}`;
  const base = { repo_root: repo, branch: spec.branch, head: "9ab44d1", upstream: `origin/${spec.branch}`, ahead: 2, behind: 0 };
  if (spec.n === 7) {
    const files = [
      f("apps/api/src/server.ts", "modified", 6, 4),
      f("packages/auth/src/legacy-compat.ts", "untracked", 58, 0),
      f("apps/api/src/routes/login.ts", "modified", 9, 7),
      f("apps/api/src/routes/logout.ts", "modified", 3, 2),
      f("apps/api/src/routes/me.ts", "modified", 2, 2),
      f("apps/api/package.json", "modified", 1, 0),
      f("pnpm-lock.yaml", "modified", 22, 3),
      f("docs/auth-flow.png", "modified", 0, 0, true),
    ];
    const diffs: Record<string, string> = { "apps/api/src/server.ts": SERVER_DIFF };
    for (const x of files.slice(1)) if (!x.binary) diffs[x.path] = genericDiff(x.path, Math.min(x.unstaged_added, 40), Math.min(x.unstaged_removed, 20));
    // A lockfile churn long enough to hit the UI's diff bound (truncation notice).
    diffs["pnpm-lock.yaml"] = genericDiff("pnpm-lock.yaml", 3000, 3);
    return { status: { ...base, files }, diffs };
  }
  const n = spec.stat?.[0] ?? (spec.kind === "working" || spec.kind === "needs" ? 3 : 0);
  const added = spec.stat?.[1] ?? 24;
  const removed = spec.stat?.[2] ?? 6;
  const files: GitFileChange[] = [];
  const diffs: Record<string, string> = {};
  for (let i = 0; i < n; i++) {
    const path = i === 0 ? `src/${spec.name}.ts` : `src/lib/part-${i}.ts`;
    const a = Math.max(1, Math.round(added / n));
    const r = Math.round(removed / n);
    files.push(f(path, "modified", a, r));
    diffs[path] = genericDiff(path, Math.min(a, 400), Math.min(r, 60));
  }
  return { status: { ...base, files }, diffs };
}

export function diffOf(t: Tree, path: string, side: GitDiff["side"], maxBytes = 1 << 20): GitDiff {
  const file = t.status.files.find((x) => x.path === path);
  if (!file) throw { code: "not_found", message: `No change to ${path}` };
  if (file.binary) return { path, side, text: "", truncated: false, binary: true };
  const full = t.diffs[path] ?? "";
  if (full.length <= maxBytes) return { path, side, text: full, truncated: false, binary: false };
  const cut = full.lastIndexOf("\n", maxBytes);
  return { path, side, text: full.slice(0, cut + 1), truncated: true, binary: false };
}
