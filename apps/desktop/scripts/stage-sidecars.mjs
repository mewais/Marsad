// Build the agent + shim and stage them as Tauri "externalBin" sidecars so
// `tauri build` bundles them INSIDE the installer (next to the app exe at runtime,
// where find_agent_bin / shim_binary discover them — no separate install, no env vars).
import { execSync } from "node:child_process";
import { mkdirSync, copyFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..", "..", "..");          // apps/desktop/scripts -> repo root
const binDir = join(here, "..", "src-tauri", "binaries");
const exe = process.platform === "win32" ? ".exe" : "";

// Host target triple (what `tauri build` builds for).
const triple = execSync("rustc -vV", { encoding: "utf8" })
  .match(/^host:\s*(.+)$/m)[1]
  .trim();

console.log(`[stage-sidecars] target triple: ${triple}`);
execSync("cargo build --release -p marsad-agent -p marsad-winshim", {
  cwd: repoRoot,
  stdio: "inherit",
});

mkdirSync(binDir, { recursive: true });
for (const name of ["marsad-agent", "marsad-winshim"]) {
  const from = join(repoRoot, "target", "release", `${name}${exe}`);
  if (!existsSync(from)) throw new Error(`missing built binary: ${from}`);
  const to = join(binDir, `${name}-${triple}${exe}`);
  copyFileSync(from, to);
  console.log(`[stage-sidecars]   ${name}${exe} -> binaries/${name}-${triple}${exe}`);
}
console.log("[stage-sidecars] done");
