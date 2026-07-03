/**
 * U2 — THE KILL-LIST GREP GATE, as a test.
 *
 * docs/specs/dynamic-capabilities.md §5 defines this as a CI grep script;
 * this test reimplements the exact same checks with `fs` so the gate lives
 * in the Vitest suite itself (per the U2 task's requirement) rather than a
 * separate shell script that could silently stop being run.
 *
 * Quoted verbatim from the spec:
 *
 *   # 1. No preset catalogs or the models module anywhere in UI/bridge/viewmodel:
 *   git grep -nE 'SLASH_CATALOG|bridge/models|all_slash_entries|slash_catalog' \
 *     -- apps/desktop/src crates/marsad-viewmodel/src            # MUST be empty
 *   # 2. No hardcoded claude model ids in the UI or the Tauri bridge:
 *   git grep -nE 'claude-(opus|sonnet|haiku)-[0-9]' \
 *     -- apps/desktop/src apps/desktop/src-tauri/src             # MUST be empty
 *   # 3. No client on-disk knowledge left in the bridge:
 *   git grep -n '\.claude/projects' -- apps/desktop/src-tauri/src  # MUST be empty
 *
 * "Model-id literals remain legal ONLY in
 * `crates/marsad-core/src/discovery.rs` (the litellm/hand pricing table —
 * U4 keeps it for historical backfill, labeled "estimated") and in
 * tests/fixtures."
 *
 * "Kill list (the preset artifacts that must be gone after U2):
 * `apps/desktop/src/bridge/models.ts` (whole file), `api.ts:581-615`
 * (`SLASH_CATALOG` + `filterSlash` fallback arm),
 * `crates/marsad-viewmodel/src/lib.rs:545-608` (`SlashEntry` catalog fns;
 * the `SlashEntry` type itself dies with them if nothing else uses it),
 * `src-tauri/bridge.rs:127-190` (`list_sessions` +
 * `parse_remote_session_list`), and the preset imports/usages at
 * `NewSession.tsx:8,73,503`, `Settings.tsx:16,1160,1194`,
 * `Config.tsx:7,59,73`, `Composer.tsx:25-26,71-72` and `Activity.tsx:553`
 * fallback comments/branches."
 *
 * This test FAILS today — the presets exist. That IS the red signal for U2.
 */
import { describe, it, expect } from "vitest";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
// tests/ -> apps/desktop -> apps -> <repo root>
const REPO_ROOT = path.resolve(__dirname, "..", "..", "..");

const DESKTOP_SRC = path.join(REPO_ROOT, "apps", "desktop", "src");
const DESKTOP_SRC_TAURI = path.join(REPO_ROOT, "apps", "desktop", "src-tauri", "src");
const VIEWMODEL_SRC = path.join(REPO_ROOT, "crates", "marsad-viewmodel", "src");

interface Hit {
  file: string;
  line: number;
  text: string;
}

/** Recursively collect files under `dir` (source-only extensions). */
function walk(dir: string, exts: string[]): string[] {
  if (!fs.existsSync(dir)) return [];
  const out: string[] = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      out.push(...walk(full, exts));
    } else if (exts.some((e) => entry.name.endsWith(e))) {
      out.push(full);
    }
  }
  return out;
}

/** Grep `pattern` across `files`, returning every matching line (git-grep-equivalent). */
function grep(files: string[], pattern: RegExp): Hit[] {
  const hits: Hit[] = [];
  for (const file of files) {
    const text = fs.readFileSync(file, "utf8");
    const lines = text.split("\n");
    lines.forEach((line, i) => {
      // Reset lastIndex for global-flagged patterns reused across lines.
      pattern.lastIndex = 0;
      if (pattern.test(line)) {
        hits.push({ file: path.relative(REPO_ROOT, file), line: i + 1, text: line.trim() });
      }
    });
  }
  return hits;
}

function formatHits(hits: Hit[]): string {
  return hits.map((h) => `${h.file}:${h.line}: ${h.text}`).join("\n");
}

const TS_EXTS = [".ts", ".tsx"];
const RS_EXTS = [".rs"];

describe("U2 kill-list grep gate (dynamic-capabilities.md §5)", () => {
  it("grep #1: no SLASH_CATALOG | bridge/models | all_slash_entries | slash_catalog in apps/desktop/src or crates/marsad-viewmodel/src", () => {
    const files = [...walk(DESKTOP_SRC, TS_EXTS), ...walk(VIEWMODEL_SRC, RS_EXTS)];
    const hits = grep(files, /SLASH_CATALOG|bridge\/models|all_slash_entries|slash_catalog/);
    expect(hits, `kill-list violations:\n${formatHits(hits)}`).toEqual([]);
  });

  it("grep #2: no hardcoded claude-(opus|sonnet|haiku)-<digit> model ids in apps/desktop/src or apps/desktop/src-tauri/src", () => {
    const files = [...walk(DESKTOP_SRC, TS_EXTS), ...walk(DESKTOP_SRC_TAURI, RS_EXTS)];
    const hits = grep(files, /claude-(opus|sonnet|haiku)-[0-9]/);
    expect(hits, `kill-list violations:\n${formatHits(hits)}`).toEqual([]);
  });

  it("grep #3: no '.claude/projects' client on-disk knowledge left in apps/desktop/src-tauri/src", () => {
    const files = walk(DESKTOP_SRC_TAURI, RS_EXTS);
    const hits = grep(files, /\.claude\/projects/);
    expect(hits, `kill-list violations:\n${formatHits(hits)}`).toEqual([]);
  });

  it("apps/desktop/src/bridge/models.ts no longer exists (whole-file kill)", () => {
    const modelsFile = path.join(DESKTOP_SRC, "bridge", "models.ts");
    expect(fs.existsSync(modelsFile), `${path.relative(REPO_ROOT, modelsFile)} must be deleted`).toBe(false);
  });

  it("src-tauri/bridge.rs no longer defines list_sessions or parse_remote_session_list", () => {
    const bridgeRs = path.join(DESKTOP_SRC_TAURI, "bridge.rs");
    const text = fs.existsSync(bridgeRs) ? fs.readFileSync(bridgeRs, "utf8") : "";
    expect(text).not.toMatch(/fn list_sessions/);
    expect(text).not.toMatch(/fn parse_remote_session_list/);
  });

  it("crates/marsad-viewmodel/src/lib.rs no longer defines SlashEntry / all_slash_entries / slash_catalog", () => {
    const libRs = path.join(VIEWMODEL_SRC, "lib.rs");
    const text = fs.existsSync(libRs) ? fs.readFileSync(libRs, "utf8") : "";
    expect(text).not.toMatch(/struct SlashEntry/);
    expect(text).not.toMatch(/fn all_slash_entries/);
    expect(text).not.toMatch(/fn slash_catalog/);
  });
});
