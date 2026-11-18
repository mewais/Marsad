#!/usr/bin/env bash
# Gather installers + standalone helper binaries into one dir and write SHA256SUMS.
# usage: collect-release.sh <downloaded-artifacts-dir> <out-dir>
set -euo pipefail
in="$1"; out="$2"
mkdir -p "$out"
# Installers (artifact names bundle-*); skip anything that is not a final installer.
find "$in" -type f \( -name '*.deb' -o -name '*.rpm' -o -name '*.AppImage' -o -name '*-setup.exe' -o -name '*.msi' -o -name '*.dmg' \) -exec cp {} "$out/" \;
# Standalone helpers: agent-<triple>/marsad-agent[.exe] -> marsad-agent-<triple>[.exe]
for d in "$in"/agent-*/; do
  t="$(basename "$d")"; t="${t#agent-}"
  for f in "$d"marsad-agent*; do
    ext=""; case "$f" in *.exe) ext=".exe";; esac
    cp "$f" "$out/marsad-agent-$t$ext"
  done
done
(cd "$out" && sha256sum * > SHA256SUMS.tmp && mv SHA256SUMS.tmp SHA256SUMS && cat SHA256SUMS)
