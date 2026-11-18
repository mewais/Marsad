#!/usr/bin/env bash
# Regenerate apps/desktop/src-tauri/icons/* from the product mark.
# Source SVGs live in apps/desktop/src-tauri/icons-src/ (full + small-size variant).
# Requires ImageMagick (convert) and the Tauri CLI (pnpm install in apps/desktop).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/apps/desktop/src-tauri/icons-src"
out="$root/apps/desktop/src-tauri/icons"
cd "$root/apps/desktop"
convert -background none -density 384 "$src/icon.svg" -resize 1024x1024 PNG32:"$src/icon-1024.png"
pnpm tauri icon "$src/icon-1024.png" -o "$out"
# Small sizes use the heavier-stroke variant so the tower stays legible.
for s in 16 24 32 48 64; do
  convert -background none -density 384 "$src/icon-small.svg" -resize ${s}x${s} PNG32:"$out/${s}x${s}.png"
done
# Rebuild the .ico with hand-tuned small frames + downsampled large ones.
convert "$out/16x16.png" "$out/24x24.png" "$out/32x32.png" "$out/48x48.png" "$out/64x64.png" \
  \( "$src/icon-1024.png" -resize 128x128 \) \( "$src/icon-1024.png" -resize 256x256 \) "$out/icon.ico"
rm -f "$out/16x16.png" "$out/24x24.png" "$out/48x48.png" "$src/icon-1024.png"
echo "icons regenerated in $out"
