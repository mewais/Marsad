// Build marsad-agent (daemon + shim + mcp entry in one binary) and stage it as a Tauri
// "externalBin" sidecar (src-tauri/tauri.bundle.conf.json) so `tauri build` bundles it next to
// the app binary, where marsad-client finds it. Binaries for other host targets (remote hosts)
// come from $MARSAD_AGENT_BIN_DIR/<triple>/ at runtime.
//
// Usage:
//   node scripts/stage-sidecars.mjs                              # host target
//   node scripts/stage-sidecars.mjs --target <triple>           # cross (e.g. x86_64-pc-windows-msvc)
// For a windows-msvc target on a non-Windows host it uses `cargo xwin` (needs
// cargo-xwin + clang/lld installed).
import { execSync } from "node:child_process";
import { mkdirSync, copyFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..", "..", "..");
const binDir = join(here, "..", "src-tauri", "binaries");

const tIdx = process.argv.indexOf("--target");
const host = execSync("rustc -vV", { encoding: "utf8" }).match(/^host:\s*(.+)$/m)[1].trim();
const triple = tIdx !== -1 ? process.argv[tIdx + 1] : host;
const exe = triple.includes("windows") ? ".exe" : "";
const crossWinFromUnix =
  triple.includes("windows-msvc") && process.platform !== "win32";
const cargo = crossWinFromUnix ? "cargo xwin build" : "cargo build";
const targetFlag = tIdx !== -1 ? `--target ${triple}` : "";
const outDir = tIdx !== -1 ? join("target", triple, "release") : join("target", "release");

console.log(`[stage-sidecars] target=${triple} cargo="${cargo}"`);
execSync(`${cargo} --release ${targetFlag} -p marsad-agent`, {
  cwd: repoRoot,
  stdio: "inherit",
});

mkdirSync(binDir, { recursive: true });
for (const name of ["marsad-agent"]) {
  const from = join(repoRoot, outDir, `${name}${exe}`);
  if (!existsSync(from)) throw new Error(`missing built binary: ${from}`);
  const to = join(binDir, `${name}-${triple}${exe}`);
  copyFileSync(from, to);
  console.log(`[stage-sidecars]   ${name}${exe} -> binaries/${name}-${triple}${exe}`);
}
console.log("[stage-sidecars] done");
