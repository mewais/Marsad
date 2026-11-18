#!/usr/bin/env bash
# Stage everything `tauri build --config src-tauri/tauri.bundle.conf.json` bundles:
#   src-tauri/binaries/marsad-agent-<app-triple>[.exe]            sidecar (externalBin)
#   src-tauri/binaries/agents/<helper-triple>/marsad-agent[.exe]  resources -> <resources>/agents/...
#
# usage: stage-bundle.sh <helpers-dir> <app-triple> <sidecar-source-triple>
#   <helpers-dir>   holds <triple>/marsad-agent[.exe] for every helper target
#   <app-triple>    the Rust target triple the desktop app is built for
#   <sidecar-source-triple>  which helper build to use as the sidecar (own platform; a static
#                   musl build serves the gnu app on Linux)
set -euo pipefail
helpers="$1"; app="$2"; side="$3"
root="$(cd "$(dirname "$0")/.." && pwd)"
bin="$root/apps/desktop/src-tauri/binaries"
rm -rf "$bin"; mkdir -p "$bin/agents"
for d in "$helpers"/*/; do
  t="$(basename "$d")"
  mkdir -p "$bin/agents/$t"
  cp "$d"/marsad-agent* "$bin/agents/$t/"
  chmod +x "$bin/agents/$t"/marsad-agent*
done
exe=""; case "$app" in *windows*) exe=".exe";; esac
src="$bin/agents/$side/marsad-agent$exe"
[ -f "$src" ] || { echo "missing helper build for sidecar: $src" >&2; exit 1; }
cp "$src" "$bin/marsad-agent-$app$exe"
find "$bin" -type f | sort
